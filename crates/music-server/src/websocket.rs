use std::time::Duration;

use axum::extract::State;
use axum::extract::ws::{Message, WebSocket, WebSocketUpgrade};
use axum::response::{IntoResponse, Response};
use futures_util::stream::{SplitSink, SplitStream};
use futures_util::{SinkExt, StreamExt};
use music_application::playback::{
    ClientRegistration, ConnectionId, PlaybackActorHandle, PlaybackPublication,
    ResolvedPlaybackCommand,
};
use music_domain::{DomainEvent, PlaybackCommand};
use music_protocol::{ClientAction, ServerMessage};
use tokio::sync::{broadcast, mpsc, watch};
use tokio::task::JoinHandle;
use tokio_util::sync::CancellationToken;

use crate::http::HttpState;
use crate::playback_projection::{canonical_state, guest_state, legacy_state};

const ABSOLUTE_VOLUME_PROTOCOL_VERSION: i64 = 2;
const MAX_WEBSOCKET_MESSAGE_BYTES: usize = 64 * 1024;
const SEND_TIMEOUT: Duration = Duration::from_secs(5);
const CLOSE_TIMEOUT: Duration = Duration::from_secs(1);
const DISCONNECT_TIMEOUT: Duration = Duration::from_secs(5);
const WRITER_COMMAND_CAPACITY: usize = 16;

#[derive(Debug, Clone, Default, Eq, PartialEq)]
struct SessionProjection {
    client_id: Option<String>,
    protocol_version: i64,
}

#[derive(Debug)]
enum WriterCommand {
    Error(&'static str),
    Pong(axum::body::Bytes),
}

pub async fn websocket_upgrade(
    State(state): State<HttpState>,
    upgrade: WebSocketUpgrade,
) -> Response {
    let Some(playback) = state.playback else {
        return upgrade.on_upgrade(unavailable_session).into_response();
    };
    upgrade
        .max_message_size(MAX_WEBSOCKET_MESSAGE_BYTES)
        .max_frame_size(MAX_WEBSOCKET_MESSAGE_BYTES)
        .on_upgrade(move |socket| websocket_session(socket, playback))
        .into_response()
}

async fn unavailable_session(mut socket: WebSocket) {
    let _ = send_server_message(
        &mut socket,
        &ServerMessage::Error {
            detail: "The playback service is unavailable.".to_owned(),
            code: None,
        },
        true,
    )
    .await;
    let _ = tokio::time::timeout(CLOSE_TIMEOUT, socket.close()).await;
}

async fn websocket_session(socket: WebSocket, playback: PlaybackActorHandle) {
    let connection_id = match playback.open_connection().await {
        Ok(connection_id) => connection_id,
        Err(_) => {
            unavailable_session(socket).await;
            return;
        }
    };
    let initial = match playback.snapshot().await {
        Ok(publication) => publication,
        Err(_) => {
            let _ = playback.disconnect(connection_id).await;
            unavailable_session(socket).await;
            return;
        }
    };
    let publications = playback.subscribe_state();
    let events = playback.subscribe_events();
    let (projection_tx, projection_rx) = watch::channel(SessionProjection::default());
    let (writer_tx, writer_rx) = mpsc::channel(WRITER_COMMAND_CAPACITY);
    let cancellation = CancellationToken::new();
    let writer_cancellation = cancellation.clone();
    let (sink, stream) = socket.split();
    let mut writer = tokio::spawn(async move {
        let result = writer_loop(
            sink,
            publications,
            events,
            projection_rx,
            writer_rx,
            initial,
            writer_cancellation.clone(),
        )
        .await;
        writer_cancellation.cancel();
        result
    });

    reader_loop(
        stream,
        &playback,
        connection_id,
        projection_tx,
        writer_tx,
        cancellation.clone(),
    )
    .await;
    cancellation.cancel();
    finish_writer(&mut writer).await;
    match tokio::time::timeout(DISCONNECT_TIMEOUT, playback.disconnect(connection_id)).await {
        Ok(Ok(_)) => {}
        Ok(Err(_)) => tracing::warn!(
            connection_id = connection_id.get(),
            "playback disconnect cleanup failed"
        ),
        Err(_) => tracing::warn!(
            connection_id = connection_id.get(),
            "playback disconnect cleanup timed out"
        ),
    }
}

async fn reader_loop(
    mut stream: SplitStream<WebSocket>,
    playback: &PlaybackActorHandle,
    connection_id: ConnectionId,
    projection: watch::Sender<SessionProjection>,
    writer: mpsc::Sender<WriterCommand>,
    cancellation: CancellationToken,
) {
    loop {
        let message = tokio::select! {
            () = cancellation.cancelled() => break,
            message = stream.next() => message,
        };
        let Some(message) = message else {
            break;
        };
        let message = match message {
            Ok(message) => message,
            Err(_) => break,
        };
        match message {
            Message::Text(text) => {
                if let Ok(action) = serde_json::from_str::<ClientAction>(&text) {
                    handle_guest_action(
                        action,
                        playback,
                        connection_id,
                        &projection,
                        &writer,
                        &cancellation,
                    )
                    .await;
                } else {
                    queue_writer(
                        &writer,
                        WriterCommand::Error("invalid action"),
                        &cancellation,
                    )
                    .await;
                }
            }
            Message::Binary(bytes) => {
                if let Ok(action) = serde_json::from_slice::<ClientAction>(&bytes) {
                    handle_guest_action(
                        action,
                        playback,
                        connection_id,
                        &projection,
                        &writer,
                        &cancellation,
                    )
                    .await;
                } else {
                    queue_writer(
                        &writer,
                        WriterCommand::Error("invalid action"),
                        &cancellation,
                    )
                    .await;
                }
            }
            Message::Ping(bytes) => {
                queue_writer(&writer, WriterCommand::Pong(bytes), &cancellation).await;
            }
            Message::Pong(_) => {}
            Message::Close(_) => break,
        }
    }
}

async fn handle_guest_action(
    action: ClientAction,
    playback: &PlaybackActorHandle,
    connection_id: ConnectionId,
    projection: &watch::Sender<SessionProjection>,
    writer: &mpsc::Sender<WriterCommand>,
    cancellation: &CancellationToken,
) {
    match action {
        ClientAction::Register {
            name,
            client_id,
            protocol_version,
        } => {
            let session = SessionProjection {
                client_id: Some(client_id.as_str().to_owned()),
                protocol_version: protocol_version.get(),
            };
            let registration = ClientRegistration {
                client_id: client_id.into_inner(),
                name: name.into_inner(),
                // Remembered/default-output ownership is implemented in the
                // authenticated device phase. Guest registration itself must
                // never silently designate a new output.
                is_default_output: false,
            };
            if playback
                .register_connection(connection_id, registration)
                .await
                .is_ok()
            {
                projection.send_replace(session);
            } else {
                queue_writer(
                    writer,
                    WriterCommand::Error("device registration failed"),
                    cancellation,
                )
                .await;
            }
        }
        ClientAction::PositionReport { position_ms } => {
            let client_id = projection.borrow().client_id.clone();
            let Some(client_id) = client_id else {
                queue_writer(
                    writer,
                    WriterCommand::Error("register before reporting position"),
                    cancellation,
                )
                .await;
                return;
            };
            let result = playback
                .execute(ResolvedPlaybackCommand::direct(
                    PlaybackCommand::ReportPosition {
                        device_id: client_id,
                        position_ms: u64::try_from(position_ms.get()).unwrap_or_default(),
                    },
                ))
                .await;
            if result.is_err() {
                queue_writer(
                    writer,
                    WriterCommand::Error("only active outputs may report position"),
                    cancellation,
                )
                .await;
            }
        }
        _ => {
            queue_writer(
                writer,
                WriterCommand::Error("guest sessions cannot mutate state — please sign in"),
                cancellation,
            )
            .await;
        }
    }
}

async fn writer_loop(
    mut sink: SplitSink<WebSocket, Message>,
    mut publications: watch::Receiver<PlaybackPublication>,
    mut events: broadcast::Receiver<DomainEvent>,
    mut projection: watch::Receiver<SessionProjection>,
    mut commands: mpsc::Receiver<WriterCommand>,
    initial: PlaybackPublication,
    cancellation: CancellationToken,
) -> Result<(), ()> {
    let initial_projection = { projection.borrow().clone() };
    send_projected_state(&mut sink, &initial, &initial_projection, true, true).await?;
    loop {
        tokio::select! {
            biased;
            () = cancellation.cancelled() => break,
            command = commands.recv() => {
                let Some(command) = command else { break; };
                match command {
                    WriterCommand::Error(detail) => {
                        send_server_message(
                            &mut sink,
                            &ServerMessage::Error { detail: detail.to_owned(), code: None },
                            true,
                        ).await?;
                    }
                    WriterCommand::Pong(bytes) => send_raw(&mut sink, Message::Pong(bytes)).await?,
                }
            }
            result = publications.changed() => {
                if result.is_err() { break; }
                let publication = publications.borrow_and_update().clone();
                let session = { projection.borrow().clone() };
                send_projected_state(
                    &mut sink,
                    &publication,
                    &session,
                    false,
                    false,
                ).await?;
            }
            result = projection.changed() => {
                if result.is_err() { break; }
                let session = projection.borrow_and_update().clone();
                let publication = { publications.borrow().clone() };
                send_projected_state(&mut sink, &publication, &session, false, false).await?;
            }
            event = events.recv() => {
                match event {
                    Ok(DomainEvent::SfxFired { soundboard_id, item_path, volume }) => {
                        send_server_message(
                            &mut sink,
                            &ServerMessage::SfxFired {
                                soundboard_id,
                                item_path,
                                volume: volume.get(),
                            },
                            true,
                        ).await?;
                    }
                    Ok(DomainEvent::LoopStarted(_) | DomainEvent::LoopStopped { .. }) => {}
                    Err(broadcast::error::RecvError::Lagged(skipped)) => {
                        tracing::warn!(skipped, "disconnecting WebSocket client that lagged transient events");
                        return Err(());
                    }
                    Err(broadcast::error::RecvError::Closed) => break,
                }
            }
        }
    }
    let _ = tokio::time::timeout(CLOSE_TIMEOUT, sink.send(Message::Close(None))).await;
    Ok(())
}

async fn send_projected_state(
    sink: &mut SplitSink<WebSocket, Message>,
    publication: &PlaybackPublication,
    session: &SessionProjection,
    snapshot: bool,
    assume_absolute: bool,
) -> Result<(), ()> {
    let canonical = canonical_state(publication).map_err(|_| ())?;
    let absolute = assume_absolute || session.protocol_version >= ABSOLUTE_VOLUME_PROTOCOL_VERSION;
    let projected = if absolute {
        canonical
    } else {
        legacy_state(canonical)
    };
    let projected = guest_state(projected, session.client_id.as_deref());
    let message = if snapshot {
        ServerMessage::StateSnapshot {
            your_device_id: String::new(),
            state: projected,
        }
    } else {
        ServerMessage::StateChanged { state: projected }
    };
    send_server_message(sink, &message, absolute).await
}

async fn send_server_message<S>(
    sink: &mut S,
    message: &ServerMessage,
    absolute_volume: bool,
) -> Result<(), ()>
where
    S: futures_util::Sink<Message> + Unpin,
{
    let mut value = serde_json::to_value(message).map_err(|_| ())?;
    if !absolute_volume
        && let Some(state) = value
            .get_mut("state")
            .and_then(serde_json::Value::as_object_mut)
    {
        state.remove("default_device_volume");
    }
    let payload = serde_json::to_string(&value).map_err(|_| ())?;
    send_raw(sink, Message::Text(payload.into())).await
}

async fn send_raw<S>(sink: &mut S, message: Message) -> Result<(), ()>
where
    S: futures_util::Sink<Message> + Unpin,
{
    match tokio::time::timeout(SEND_TIMEOUT, sink.send(message)).await {
        Ok(Ok(())) => Ok(()),
        Ok(Err(_)) | Err(_) => Err(()),
    }
}

async fn queue_writer(
    writer: &mpsc::Sender<WriterCommand>,
    command: WriterCommand,
    cancellation: &CancellationToken,
) {
    match tokio::time::timeout(SEND_TIMEOUT, writer.send(command)).await {
        Ok(Ok(())) => {}
        Ok(Err(_)) | Err(_) => cancellation.cancel(),
    }
}

async fn finish_writer(writer: &mut JoinHandle<Result<(), ()>>) {
    if tokio::time::timeout(CLOSE_TIMEOUT, &mut *writer)
        .await
        .is_err()
    {
        writer.abort();
        let _ = writer.await;
    }
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;
    use std::error::Error;
    use std::io;
    use std::path::Path;
    use std::time::Duration;

    use futures_util::{SinkExt, StreamExt};
    use music_protocol::ServerMessage;
    use tempfile::tempdir;
    use tokio::net::TcpStream;
    use tokio::sync::oneshot;
    use tokio_tungstenite::tungstenite::Message as TungsteniteMessage;
    use tokio_tungstenite::{MaybeTlsStream, WebSocketStream, connect_async};

    use crate::{AppConfig, AppRuntime, RuntimeError};

    fn runtime_config(root: &Path) -> Result<AppConfig, RuntimeError> {
        AppConfig::from_values(&BTreeMap::from([
            (
                "DATABASE_URL".to_owned(),
                format!("sqlite:///{}", root.join("app.db").display()),
            ),
            (
                "MUSIC_DIR".to_owned(),
                root.join("music").display().to_string(),
            ),
            (
                "SFX_LIBRARY_DIR".to_owned(),
                root.join("sfx").display().to_string(),
            ),
            (
                "MODES_DIR".to_owned(),
                root.join("modes").display().to_string(),
            ),
            (
                "STATIC_DIR".to_owned(),
                root.join("missing-static").display().to_string(),
            ),
        ]))
        .map_err(Into::into)
    }

    async fn next_protocol_message(
        socket: &mut WebSocketStream<MaybeTlsStream<TcpStream>>,
    ) -> Result<ServerMessage, Box<dyn Error>> {
        loop {
            let incoming = tokio::time::timeout(Duration::from_secs(2), socket.next()).await?;
            let Some(incoming) = incoming else {
                return Err(io::Error::new(
                    io::ErrorKind::UnexpectedEof,
                    "WebSocket closed before the expected message",
                )
                .into());
            };
            match incoming? {
                TungsteniteMessage::Text(text) => {
                    return Ok(serde_json::from_str(text.as_str())?);
                }
                TungsteniteMessage::Binary(bytes) => {
                    return Ok(serde_json::from_slice(&bytes)?);
                }
                TungsteniteMessage::Ping(bytes) => {
                    socket.send(TungsteniteMessage::Pong(bytes)).await?;
                }
                TungsteniteMessage::Pong(_) | TungsteniteMessage::Frame(_) => {}
                TungsteniteMessage::Close(_) => {
                    return Err(io::Error::new(
                        io::ErrorKind::UnexpectedEof,
                        "WebSocket closed before the expected message",
                    )
                    .into());
                }
            }
        }
    }

    #[tokio::test]
    async fn guest_socket_registers_but_cannot_control_playback() -> Result<(), Box<dyn Error>> {
        let directory = tempdir()?;
        let runtime = AppRuntime::start(runtime_config(directory.path())?).await?;
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await?;
        let address = listener.local_addr()?;
        let (shutdown_tx, shutdown_rx) = oneshot::channel();
        let server = tokio::spawn(runtime.run(listener, async move {
            let _ = shutdown_rx.await;
            Ok(())
        }));
        let (mut socket, _) = connect_async(format!("ws://{address}/api/ws")).await?;

        let initial = next_protocol_message(&mut socket).await?;
        assert!(matches!(
            initial,
            ServerMessage::StateSnapshot {
                your_device_id,
                state,
            } if your_device_id.is_empty() && state.connected_devices.is_empty()
        ));

        socket
            .send(TungsteniteMessage::Text(
                serde_json::json!({"type":"pause"}).to_string().into(),
            ))
            .await?;
        assert!(matches!(
            next_protocol_message(&mut socket).await?,
            ServerMessage::Error { detail, .. } if detail.contains("guest sessions")
        ));

        socket
            .send(TungsteniteMessage::Text(
                serde_json::json!({
                    "type":"register",
                    "name":"Living room TV",
                    "client_id":"living-room-tv",
                    "protocol_version":2
                })
                .to_string()
                .into(),
            ))
            .await?;
        let mut saw_self_projection = false;
        for _ in 0..4 {
            if let ServerMessage::StateChanged { state } =
                next_protocol_message(&mut socket).await?
                && state.device_volumes.contains_key("living-room-tv")
                && state.connected_devices.is_empty()
            {
                saw_self_projection = true;
                break;
            }
        }
        assert!(saw_self_projection);

        socket
            .send(TungsteniteMessage::Text(
                serde_json::json!({"type":"position_report","position_ms":1234})
                    .to_string()
                    .into(),
            ))
            .await?;
        assert!(matches!(
            next_protocol_message(&mut socket).await?,
            ServerMessage::Error { detail, .. } if detail.contains("active outputs")
        ));

        socket.close(None).await?;
        let _ = shutdown_tx.send(());
        server.await??;
        Ok(())
    }
}
