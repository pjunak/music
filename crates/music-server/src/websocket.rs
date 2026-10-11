use std::sync::Arc;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use axum::extract::State;
use axum::extract::ws::{Message, WebSocket, WebSocketUpgrade};
use axum::http::HeaderMap;
use axum::response::{IntoResponse, Response};
use futures_util::stream::{SplitSink, SplitStream};
use futures_util::{SinkExt, StreamExt};
use music_application::ErrorChain;
use music_application::auth::{SecretSessionToken, SessionLookup, SessionTouch, UnixSeconds};
use music_application::playback::{
    CatalogCommandError, CatalogCommands, CatalogRequest, ClientRegistration, ConnectionId,
    InterruptFade, PlaybackActorError, PlaybackActorHandle, PlaybackPublication,
    ResolvedPlaybackCommand,
};
use music_domain::{
    CrossfadeType as DomainCrossfadeType, DomainEvent, LoopMode as DomainLoopMode, LoopingSfx,
    PlaybackCommand, ShuffleMode as DomainShuffleMode, TrackId, UnitInterval as DomainUnitInterval,
};
use music_protocol::{ClientAction, ErrorCode, ServerMessage};
use tokio::sync::{broadcast, mpsc, watch};
use tokio::task::JoinHandle;
use tokio::time::MissedTickBehavior;
use tokio_util::sync::CancellationToken;

use crate::auth::{CurrentSession, RuntimeAuth, optional_session};
use crate::devices::RuntimeDevices;
use crate::http::HttpState;
use crate::playback_projection::{canonical_state, guest_state, legacy_state};

const ABSOLUTE_VOLUME_PROTOCOL_VERSION: i64 = 2;
const MAX_WEBSOCKET_MESSAGE_BYTES: usize = 64 * 1024;
const SEND_TIMEOUT: Duration = Duration::from_secs(5);
const CLOSE_TIMEOUT: Duration = Duration::from_secs(1);
const DISCONNECT_TIMEOUT: Duration = Duration::from_secs(5);
const WRITER_COMMAND_CAPACITY: usize = 16;
#[cfg(not(test))]
const SESSION_RECHECK_INTERVAL: Duration = Duration::from_secs(10);
#[cfg(test)]
const SESSION_RECHECK_INTERVAL: Duration = Duration::from_millis(50);

#[derive(Debug, Clone, Default, Eq, PartialEq)]
struct SessionProjection {
    client_id: Option<String>,
    protocol_version: i64,
    authenticated: bool,
}

#[derive(Debug)]
enum WriterCommand {
    Error {
        detail: String,
        code: Option<ErrorCode>,
    },
    Pong(axum::body::Bytes),
}

#[derive(Debug)]
struct SessionAuthorization {
    token: Option<SecretSessionToken>,
    expires_at: Option<UnixSeconds>,
    last_database_check: Instant,
}

struct ReaderContext {
    playback: PlaybackActorHandle,
    connection_id: ConnectionId,
    auth: Option<Arc<RuntimeAuth>>,
    devices: Option<Arc<RuntimeDevices>>,
    catalog: Option<CatalogCommands>,
    authorization: SessionAuthorization,
    projection: watch::Sender<SessionProjection>,
    writer: mpsc::Sender<WriterCommand>,
    cancellation: CancellationToken,
}

struct WebsocketServices {
    playback: PlaybackActorHandle,
    shutdown: CancellationToken,
    auth: Option<Arc<RuntimeAuth>>,
    devices: Option<Arc<RuntimeDevices>>,
    catalog: Option<CatalogCommands>,
}

impl SessionAuthorization {
    fn from_session(session: Option<CurrentSession>) -> Self {
        let (token, expires_at) = session.map_or((None, None), |session| {
            (Some(session.token), Some(session.expires_at))
        });
        Self {
            token,
            expires_at,
            last_database_check: Instant::now(),
        }
    }

    fn authenticated(&self) -> bool {
        self.token.is_some()
    }

    fn downgrade(&mut self) {
        self.token = None;
        self.expires_at = None;
    }
}

pub async fn websocket_upgrade(
    State(state): State<HttpState>,
    headers: HeaderMap,
    upgrade: WebSocketUpgrade,
) -> Response {
    if !state.websocket_origin.allows(&headers) {
        return crate::error::ApiError::coded(
            axum::http::StatusCode::FORBIDDEN,
            "websocket_origin_denied",
            "The WebSocket origin is not allowed.",
        )
        .into_response();
    }
    let Some(playback) = state.playback.clone() else {
        return upgrade.on_upgrade(unavailable_session).into_response();
    };
    let initial_session =
        match optional_session(&state, &headers, SessionTouch::PreserveLastSeen).await {
            Ok(session) => session,
            Err(error) => return error.into_response(),
        };
    let services = WebsocketServices {
        catalog: catalog_commands(&state, &playback),
        playback,
        shutdown: state.shutdown.clone(),
        auth: state.auth.clone(),
        devices: state.devices.clone(),
    };
    upgrade
        .max_message_size(MAX_WEBSOCKET_MESSAGE_BYTES)
        .max_frame_size(MAX_WEBSOCKET_MESSAGE_BYTES)
        .on_upgrade(move |socket| websocket_session(socket, services, initial_session))
        .into_response()
}

/// Catalog-scoped actions need the library, mode and playlist owners.
fn catalog_commands(state: &HttpState, playback: &PlaybackActorHandle) -> Option<CatalogCommands> {
    let library = state.library.as_deref()?;
    Some(CatalogCommands::new(
        playback.clone(),
        library.coordinator.clone(),
        Arc::clone(&library.service),
        state.modes.clone()?,
        state.playlists.clone()?,
    ))
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

async fn websocket_session(
    socket: WebSocket,
    services: WebsocketServices,
    initial_session: Option<CurrentSession>,
) {
    let WebsocketServices {
        playback,
        shutdown,
        auth,
        devices,
        catalog,
    } = services;
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
    let authorization = SessionAuthorization::from_session(initial_session);
    let (projection_tx, projection_rx) = watch::channel(SessionProjection {
        authenticated: authorization.authenticated(),
        ..SessionProjection::default()
    });
    let (writer_tx, writer_rx) = mpsc::channel(WRITER_COMMAND_CAPACITY);
    let cancellation = shutdown.child_token();
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
        ReaderContext {
            playback: playback.clone(),
            connection_id,
            auth,
            devices,
            catalog,
            authorization,
            projection: projection_tx,
            writer: writer_tx,
            cancellation: cancellation.clone(),
        },
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

async fn reader_loop(mut stream: SplitStream<WebSocket>, mut context: ReaderContext) {
    let mut session_recheck = tokio::time::interval(SESSION_RECHECK_INTERVAL);
    session_recheck.set_missed_tick_behavior(MissedTickBehavior::Skip);
    loop {
        let message = tokio::select! {
            () = context.cancellation.cancelled() => break,
            _ = session_recheck.tick(), if context.authorization.authenticated() => {
                refresh_session_projection(&mut context).await;
                continue;
            }
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
                    handle_action(action, &mut context).await;
                } else {
                    queue_writer(
                        &context.writer,
                        WriterCommand::Error {
                            detail: "invalid action".to_owned(),
                            code: None,
                        },
                        &context.cancellation,
                    )
                    .await;
                }
            }
            Message::Binary(bytes) => {
                if let Ok(action) = serde_json::from_slice::<ClientAction>(&bytes) {
                    handle_action(action, &mut context).await;
                } else {
                    queue_writer(
                        &context.writer,
                        WriterCommand::Error {
                            detail: "invalid action".to_owned(),
                            code: None,
                        },
                        &context.cancellation,
                    )
                    .await;
                }
            }
            Message::Ping(bytes) => {
                queue_writer(
                    &context.writer,
                    WriterCommand::Pong(bytes),
                    &context.cancellation,
                )
                .await;
            }
            Message::Pong(_) => {}
            Message::Close(_) => break,
        }
    }
}

async fn handle_action(action: ClientAction, context: &mut ReaderContext) {
    refresh_session_projection(context).await;
    let ReaderContext {
        playback,
        connection_id,
        devices,
        catalog,
        authorization,
        projection,
        writer,
        cancellation,
        ..
    } = context;
    let connection_id = *connection_id;
    match action {
        ClientAction::Register {
            name,
            client_id,
            protocol_version,
        } => {
            let client_id = client_id.into_inner();
            let name = name.into_inner();
            let remembered = if let Some(devices) = devices.as_deref() {
                match devices.find(&client_id).await {
                    Ok(device) => device,
                    Err(error) => {
                        tracing::error!(error = %ErrorChain(&error), "remembered-device lookup failed during registration");
                        queue_error(writer, "device registration failed", None, cancellation).await;
                        return;
                    }
                }
            } else {
                None
            };
            let session = SessionProjection {
                client_id: Some(client_id.clone()),
                protocol_version: protocol_version.get(),
                authenticated: authorization.authenticated(),
            };
            let registration = ClientRegistration {
                client_id,
                name,
                remembered_name: remembered.as_ref().map(|device| device.name.clone()),
                is_default_output: remembered.is_some_and(|device| device.is_output),
            };
            if playback
                .register_connection(connection_id, registration)
                .await
                .is_ok()
            {
                projection.send_replace(session);
            } else {
                queue_error(writer, "device registration failed", None, cancellation).await;
            }
        }
        ClientAction::PositionReport { position_ms } => {
            let client_id = projection.borrow().client_id.clone();
            let Some(client_id) = client_id else {
                queue_error(
                    writer,
                    "register before reporting position",
                    None,
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
                queue_error(
                    writer,
                    "only active outputs may report position",
                    None,
                    cancellation,
                )
                .await;
            }
        }
        action if !authorization.authenticated() => {
            queue_error(
                writer,
                "guest sessions cannot mutate state — please sign in",
                None,
                cancellation,
            )
            .await;
            drop(action);
        }
        ClientAction::SetActiveOutputs { device_ids } => {
            match playback
                .execute(ResolvedPlaybackCommand::direct(
                    PlaybackCommand::SetActiveOutputs(device_ids),
                ))
                .await
            {
                Ok(_) => {}
                Err(PlaybackActorError::DisconnectedOutput(device_ids)) => {
                    queue_error(
                        writer,
                        format!("device(s) not connected: {}", device_ids.join(", ")),
                        None,
                        cancellation,
                    )
                    .await;
                }
                Err(error) => {
                    tracing::warn!(error = %ErrorChain(&error), "active-output selection was rejected");
                    queue_error(writer, "playback action failed", None, cancellation).await;
                }
            }
        }
        ClientAction::AmbientSkipNext { from_track_id } => {
            let expected_track_id = match from_track_id.map(TrackId::new).transpose() {
                Ok(track_id) => track_id,
                Err(_) => {
                    queue_error(writer, "track ID must be positive", None, cancellation).await;
                    return;
                }
            };
            if let Err(error) = playback.ambient_skip_next(expected_track_id).await {
                tracing::warn!(error = %ErrorChain(&error), "ambient skip was rejected");
                queue_error(writer, "playback action failed", None, cancellation).await;
            }
        }
        action => {
            let result = match playback_request(action) {
                Ok(PlaybackRequest::Direct(command)) => playback
                    .execute(ResolvedPlaybackCommand::direct(command))
                    .await
                    .map(|_| ())
                    .map_err(|error| {
                        tracing::warn!(error = %ErrorChain(&error), "authenticated playback action was rejected");
                        "playback action failed".to_owned()
                    }),
                Ok(PlaybackRequest::Catalog(request)) => match catalog.as_ref() {
                    Some(catalog) => catalog
                        .execute(&request)
                        .await
                        .map_err(|error| error.to_string()),
                    None => Err(CatalogCommandError::NotReady.to_string()),
                },
                Err(detail) => Err(detail.to_owned()),
            };
            if let Err(detail) = result {
                queue_error(writer, detail, None, cancellation).await;
            }
        }
    }
}

/// A validated playback request translated from the wire protocol.
enum PlaybackRequest {
    Direct(PlaybackCommand),
    Catalog(CatalogRequest),
}

/// Translate an authenticated action into domain values. Connection-scoped
/// actions (register, position, outputs, skip) are handled by the caller.
fn playback_request(action: ClientAction) -> Result<PlaybackRequest, &'static str> {
    use PlaybackRequest::{Catalog, Direct};
    Ok(match action {
        ClientAction::SetVolume { volume } => Direct(PlaybackCommand::SetGroupVolume(
            domain_volume(volume.get())?,
        )),
        ClientAction::Pause => Direct(PlaybackCommand::SetPlaying(false)),
        ClientAction::Resume => Direct(PlaybackCommand::SetPlaying(true)),
        ClientAction::SetDeviceVolume { device_id, volume } => {
            Direct(PlaybackCommand::SetDeviceVolume {
                device_id: device_id.into_inner(),
                volume: domain_volume(volume.get())?,
            })
        }
        ClientAction::AmbientJumpQueue { position } => Direct(PlaybackCommand::AmbientJumpQueue(
            usize::try_from(position.get()).map_err(|_| "queue position is too large")?,
        )),
        ClientAction::AmbientClearQueue => Direct(PlaybackCommand::AmbientClearQueue),
        ClientAction::AmbientSkipPrev => Direct(PlaybackCommand::AmbientSkipPrevious),
        ClientAction::AmbientSeek { position_ms } => Direct(PlaybackCommand::AmbientSeek(
            u64::try_from(position_ms.get()).map_err(|_| "position is too large")?,
        )),
        ClientAction::AmbientSetLoop { loop_mode } => {
            Direct(PlaybackCommand::AmbientSetLoop(match loop_mode {
                music_protocol::LoopMode::Off => DomainLoopMode::Off,
                music_protocol::LoopMode::Follow => DomainLoopMode::Follow,
                music_protocol::LoopMode::Queue => DomainLoopMode::Queue,
                music_protocol::LoopMode::Track => DomainLoopMode::Track,
            }))
        }
        ClientAction::AmbientSetShuffle { shuffle } => {
            Direct(PlaybackCommand::AmbientSetShuffle(match shuffle {
                music_protocol::ShuffleMode::Off => DomainShuffleMode::Off,
                music_protocol::ShuffleMode::Random => DomainShuffleMode::Random,
            }))
        }
        ClientAction::AmbientStop => Direct(PlaybackCommand::AmbientStop),
        ClientAction::SetCrossfade {
            crossfade_ms,
            crossfade_type,
        } => Direct(PlaybackCommand::SetCrossfade {
            crossfade_ms: u32::try_from(crossfade_ms.get())
                .map_err(|_| "crossfade duration is too large")?,
            crossfade_type: crossfade_type.map(|kind| match kind {
                music_protocol::CrossfadeType::Linear => DomainCrossfadeType::Linear,
                music_protocol::CrossfadeType::EqualPower => DomainCrossfadeType::EqualPower,
                music_protocol::CrossfadeType::Cut => DomainCrossfadeType::Cut,
            }),
        }),
        ClientAction::InterruptSkipNext { from_track_id } => {
            Direct(PlaybackCommand::InterruptSkipNext {
                expected_track_id: from_track_id.map(track_id).transpose()?,
            })
        }
        ClientAction::InterruptSeek { position_ms } => Direct(PlaybackCommand::InterruptSeek(
            u64::try_from(position_ms.get()).map_err(|_| "position is too large")?,
        )),
        ClientAction::CancelInterrupt => Direct(PlaybackCommand::CancelInterrupt),
        ClientAction::StopLoop { id } => Direct(PlaybackCommand::StopLoop(id.into_inner())),
        ClientAction::SetActiveMode { mode_id } => {
            Catalog(CatalogRequest::SetActiveMode(mode_id.into_inner()))
        }
        ClientAction::AmbientPlayTrack { track_id: id } => {
            Catalog(CatalogRequest::PlayTrack(track_id(id)?))
        }
        ClientAction::AmbientSetQueue { track_ids } => Catalog(CatalogRequest::SetQueue(
            track_ids
                .into_iter()
                .map(track_id)
                .collect::<Result<_, _>>()?,
        )),
        ClientAction::AmbientEnqueue {
            track_id: id,
            position,
        } => Catalog(CatalogRequest::Enqueue {
            track_id: track_id(id)?,
            position: position
                .map(|position| usize::try_from(position.max(0)).unwrap_or(usize::MAX)),
        }),
        ClientAction::AmbientPlayPlaylist {
            playlist_id,
            start_index,
        } => Catalog(CatalogRequest::PlayPlaylist {
            playlist_id,
            start_index: usize::try_from(start_index.get())
                .map_err(|_| "playlist start index is too large")?,
        }),
        ClientAction::AmbientPlayFolder { path, start_index } => {
            Catalog(CatalogRequest::PlayFolder {
                path: path.into_inner(),
                start_index: usize::try_from(start_index.get())
                    .map_err(|_| "folder start index is too large")?,
            })
        }
        ClientAction::SetActiveSoundboard { soundboard_id } => Catalog(
            CatalogRequest::SetActiveSoundboard(soundboard_id.into_inner()),
        ),
        ClientAction::SetActivePresets { preset_ids } => {
            Catalog(CatalogRequest::SetActivePresets(preset_ids))
        }
        ClientAction::FireInterruptTrack {
            duck_to,
            track_id: id,
            return_to_ambient,
            fade_in_ms,
            fade_out_ms,
        } => Catalog(CatalogRequest::InterruptTrack {
            track_id: track_id(id)?,
            fade: interrupt_fade(
                return_to_ambient,
                fade_in_ms.get(),
                fade_out_ms.get(),
                duck_to.map(|value| value.get()),
            )?,
        }),
        ClientAction::FireInterruptPlaylist {
            playlist_id,
            return_to_ambient,
            fade_in_ms,
            fade_out_ms,
            duck_to,
        } => Catalog(CatalogRequest::InterruptPlaylist {
            playlist_id,
            fade: interrupt_fade(
                return_to_ambient,
                fade_in_ms.get(),
                fade_out_ms.get(),
                duck_to.map(|value| value.get()),
            )?,
        }),
        ClientAction::FireCue { cue_id } => Catalog(CatalogRequest::FireCue(cue_id.into_inner())),
        ClientAction::FireSfx {
            soundboard_id,
            item_path,
            volume,
        } => Catalog(CatalogRequest::FireSfx {
            soundboard_id: soundboard_id.into_inner(),
            item_path: item_path.into_inner(),
            volume: domain_volume(volume.get())?,
        }),
        ClientAction::StartLoop {
            id,
            name,
            soundboard_id,
            item_path,
            interval_s,
            volume,
        } => {
            let volume = domain_volume(volume.get())?;
            Catalog(CatalogRequest::StartLoop(
                LoopingSfx::new(
                    id.into_inner(),
                    name.into_inner(),
                    soundboard_id.into_inner(),
                    item_path.into_inner(),
                    interval_s.get(),
                    volume,
                )
                .map_err(|_| "loop parameters are invalid")?,
            ))
        }
        ClientAction::Register { .. }
        | ClientAction::PositionReport { .. }
        | ClientAction::SetActiveOutputs { .. }
        | ClientAction::AmbientSkipNext { .. } => return Err("action was routed incorrectly"),
    })
}

fn track_id(value: i64) -> Result<TrackId, &'static str> {
    TrackId::new(value).map_err(|_| "track ID must be positive")
}

fn interrupt_fade(
    return_to_ambient: bool,
    fade_in_ms: i64,
    fade_out_ms: i64,
    duck_to: Option<f64>,
) -> Result<InterruptFade, &'static str> {
    Ok(InterruptFade {
        return_to_ambient,
        fade_in_ms: u32::try_from(fade_in_ms).map_err(|_| "fade duration is too large")?,
        fade_out_ms: u32::try_from(fade_out_ms).map_err(|_| "fade duration is too large")?,
        duck_to: duck_to.map(domain_volume).transpose()?,
    })
}

fn domain_volume(value: f64) -> Result<DomainUnitInterval, &'static str> {
    DomainUnitInterval::new(value).map_err(|_| "volume is outside the supported range")
}

async fn refresh_session_projection(context: &mut ReaderContext) {
    let ReaderContext {
        auth,
        authorization,
        projection,
        writer,
        cancellation,
        ..
    } = context;
    if let Some((detail, code)) = refresh_authorization(auth.as_deref(), authorization).await {
        let mut session = projection.borrow().clone();
        session.authenticated = false;
        projection.send_replace(session);
        queue_writer(
            writer,
            WriterCommand::Error {
                detail: detail.to_owned(),
                code: Some(code),
            },
            cancellation,
        )
        .await;
    }
}

async fn refresh_authorization(
    auth: Option<&RuntimeAuth>,
    authorization: &mut SessionAuthorization,
) -> Option<(&'static str, ErrorCode)> {
    let token = authorization.token.as_ref()?;
    let now = match current_unix_seconds() {
        Some(now) => now,
        None => {
            authorization.downgrade();
            return Some((
                "session could not be verified — please sign in again",
                ErrorCode::SessionRevoked,
            ));
        }
    };
    if authorization
        .expires_at
        .is_some_and(|expires_at| now >= expires_at)
    {
        authorization.downgrade();
        return Some((
            "session expired — please sign in again",
            ErrorCode::SessionExpired,
        ));
    }
    if authorization.last_database_check.elapsed() < SESSION_RECHECK_INTERVAL {
        return None;
    }
    authorization.last_database_check = Instant::now();
    let Some(auth) = auth else {
        authorization.downgrade();
        return Some((
            "session revoked — please sign in again",
            ErrorCode::SessionRevoked,
        ));
    };
    let token = token.clone();
    match auth
        .authenticate(token.expose_secret(), SessionTouch::PreserveLastSeen)
        .await
    {
        Ok(SessionLookup::Authenticated { expires_at, .. }) => {
            authorization.expires_at = Some(expires_at);
            None
        }
        Ok(SessionLookup::Expired) => {
            authorization.downgrade();
            Some((
                "session expired — please sign in again",
                ErrorCode::SessionExpired,
            ))
        }
        Ok(SessionLookup::Missing) => {
            authorization.downgrade();
            Some((
                "session revoked — please sign in again",
                ErrorCode::SessionRevoked,
            ))
        }
        Err(error) => {
            tracing::error!(error = %ErrorChain(&error), "WebSocket session recheck failed closed");
            authorization.downgrade();
            Some((
                "session could not be verified — please sign in again",
                ErrorCode::SessionRevoked,
            ))
        }
    }
}

fn current_unix_seconds() -> Option<UnixSeconds> {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .ok()
        .and_then(|duration| i64::try_from(duration.as_secs()).ok())
        .map(UnixSeconds::new)
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
                    WriterCommand::Error { detail, code } => {
                        send_server_message(
                            &mut sink,
                            &ServerMessage::Error { detail, code },
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
    let projected = if session.authenticated {
        projected
    } else {
        guest_state(projected, session.client_id.as_deref())
    };
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

async fn queue_error(
    writer: &mpsc::Sender<WriterCommand>,
    detail: impl Into<String>,
    code: Option<ErrorCode>,
    cancellation: &CancellationToken,
) {
    queue_writer(
        writer,
        WriterCommand::Error {
            detail: detail.into(),
            code,
        },
        cancellation,
    )
    .await;
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
    use std::error::Error;
    use std::fs;
    use std::io;
    use std::path::Path;
    use std::time::Duration;

    use axum::body::{Body, to_bytes};
    use axum::http::header::COOKIE;
    use axum::http::{HeaderValue, Request, StatusCode};
    use futures_util::{SinkExt, StreamExt};
    use music_application::auth::{AuthRepository, UnixSeconds};
    use music_application::devices::RememberedDeviceRepository;
    use music_application::library::ReconciliationStatus;
    use music_protocol::{ErrorCode, ServerMessage};
    use music_storage::{SqliteStorage, SqliteStorageOptions};
    use tempfile::tempdir;
    use tokio::net::TcpStream;
    use tokio::sync::oneshot;
    use tokio_tungstenite::tungstenite::Message as TungsteniteMessage;
    use tokio_tungstenite::tungstenite::client::IntoClientRequest;
    use tokio_tungstenite::{MaybeTlsStream, WebSocketStream, connect_async};
    use tower::ServiceExt;

    use crate::{AppConfig, AppRuntime, RuntimeError, TEST_PASSWORD_HASH};

    fn runtime_config(root: &Path) -> Result<AppConfig, RuntimeError> {
        crate::test_support::test_config(root, &[]).map_err(Into::into)
    }

    async fn seed_session(root: &Path, token: &str) -> Result<(), Box<dyn Error>> {
        let storage = SqliteStorage::open(SqliteStorageOptions::new(root.join("app.db"))).await?;
        let user_id = storage
            .create_user(
                "operator",
                TEST_PASSWORD_HASH,
                UnixSeconds::new(1_800_000_000),
            )
            .await?;
        AuthRepository::create_session(
            &storage,
            user_id,
            token,
            UnixSeconds::new(1_800_000_000),
            UnixSeconds::new(4_000_000_000),
        )
        .await
        .map_err(|error| -> Box<dyn Error> { error })?;
        storage.close().await;
        Ok(())
    }

    async fn seed_remembered_output(
        root: &Path,
        client_id: &str,
        name: &str,
    ) -> Result<(), Box<dyn Error>> {
        let storage = SqliteStorage::open(SqliteStorageOptions::new(root.join("app.db"))).await?;
        storage
            .upsert_device(client_id, name, true)
            .await
            .map_err(|error| -> Box<dyn Error> { error })?;
        storage.close().await;
        Ok(())
    }

    async fn next_protocol_message(
        socket: &mut WebSocketStream<MaybeTlsStream<TcpStream>>,
    ) -> Result<ServerMessage, Box<dyn Error>> {
        loop {
            let incoming = tokio::time::timeout(Duration::from_secs(5), socket.next())
                .await
                .map_err(|_| {
                    io::Error::new(
                        io::ErrorKind::TimedOut,
                        "WebSocket did not publish the next protocol message within five seconds",
                    )
                })?;
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

    async fn wait_for_protocol_message(
        socket: &mut WebSocketStream<MaybeTlsStream<TcpStream>>,
        description: &str,
        mut matches: impl FnMut(&ServerMessage) -> bool,
    ) -> Result<ServerMessage, Box<dyn Error>> {
        for _ in 0..8 {
            let message = next_protocol_message(socket).await?;
            if matches(&message) {
                return Ok(message);
            }
        }
        Err(io::Error::other(format!(
            "WebSocket did not publish {description} within eight messages"
        ))
        .into())
    }

    #[tokio::test]
    async fn websocket_origin_gate_preserves_native_and_allowed_browsers_and_rejects_cookie_requests()
    -> Result<(), Box<dyn Error>> {
        let directory = tempdir()?;
        let token = "synthetic-origin-test-session";
        seed_session(directory.path(), token).await?;
        let mut config = runtime_config(directory.path())?;
        config.allowed_origins = vec!["https://controller.example".to_owned()];
        let runtime = AppRuntime::start(config).await?;
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await?;
        let address = listener.local_addr()?;
        let (shutdown_tx, shutdown_rx) = oneshot::channel();
        let server = tokio::spawn(runtime.run(listener, async move {
            let _ = shutdown_rx.await;
            Ok(())
        }));
        let url = format!("ws://{address}/api/ws");
        let same_origin = format!("http://{address}");
        for origin in [
            None,
            Some(same_origin.as_str()),
            Some("https://controller.example"),
        ] {
            let mut request = url.as_str().into_client_request()?;
            request.headers_mut().insert(
                COOKIE,
                HeaderValue::from_str(&format!("music_session={token}"))?,
            );
            if let Some(origin) = origin {
                request
                    .headers_mut()
                    .insert("origin", HeaderValue::from_str(origin)?);
            }
            let (mut socket, response) = connect_async(request).await?;
            assert_eq!(response.status(), StatusCode::SWITCHING_PROTOCOLS);
            assert!(matches!(
                next_protocol_message(&mut socket).await?,
                ServerMessage::StateSnapshot { .. }
            ));
            socket.close(None).await?;
        }
        for origins in [
            vec!["https://unapproved.example"],
            vec!["null"],
            vec![""],
            vec!["https://controller.example/path"],
            vec!["https://controller.example@unapproved.example"],
            vec!["https://controller.example https://unapproved.example"],
            vec!["https://controller.example", "https://unapproved.example"],
        ] {
            let mut request = url.as_str().into_client_request()?;
            request.headers_mut().insert(
                COOKIE,
                HeaderValue::from_str(&format!("music_session={token}"))?,
            );
            for origin in origins {
                request
                    .headers_mut()
                    .append("origin", HeaderValue::from_str(origin)?);
            }
            match connect_async(request).await {
                Err(tokio_tungstenite::tungstenite::Error::Http(response)) => {
                    assert_eq!(response.status(), StatusCode::FORBIDDEN);
                }
                _ => return Err("unapproved Origin was not rejected before upgrade".into()),
            }
        }
        let _ = shutdown_tx.send(());
        server.await??;
        Ok(())
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

    #[tokio::test]
    async fn runtime_shutdown_closes_an_open_websocket_without_client_cooperation()
    -> Result<(), Box<dyn Error>> {
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
        let _initial = next_protocol_message(&mut socket).await?;

        let _ = shutdown_tx.send(());
        tokio::time::timeout(Duration::from_secs(5), server).await???;

        let closed = tokio::time::timeout(Duration::from_secs(5), async {
            while let Some(message) = socket.next().await {
                match message {
                    Ok(TungsteniteMessage::Close(_)) | Err(_) => return true,
                    Ok(_) => {}
                }
            }
            true
        })
        .await?;
        assert!(closed);
        Ok(())
    }

    #[tokio::test]
    async fn authenticated_socket_controls_playback_uses_remembered_defaults_and_downgrades()
    -> Result<(), Box<dyn Error>> {
        let directory = tempdir()?;
        let token = "authenticated-test-session-token";
        seed_session(directory.path(), token).await?;
        seed_remembered_output(directory.path(), "living-room", "Living Room TV").await?;
        let runtime = AppRuntime::start(runtime_config(directory.path())?).await?;
        let control_router = runtime.router()?;
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await?;
        let address = listener.local_addr()?;
        let (shutdown_tx, shutdown_rx) = oneshot::channel();
        let server = tokio::spawn(runtime.run(listener, async move {
            let _ = shutdown_rx.await;
            Ok(())
        }));
        let mut request = format!("ws://{address}/api/ws").into_client_request()?;
        request.headers_mut().insert(
            COOKIE,
            HeaderValue::from_str(&format!("music_session={token}"))?,
        );
        let (mut socket, _) = connect_async(request).await?;
        assert!(matches!(
            next_protocol_message(&mut socket).await?,
            ServerMessage::StateSnapshot { state, .. } if state.connected_devices.is_empty()
        ));

        socket
            .send(TungsteniteMessage::Text(
                serde_json::json!({
                    "type":"register",
                    "name":"TV Browser",
                    "client_id":"living-room",
                    "protocol_version":2
                })
                .to_string()
                .into(),
            ))
            .await?;
        let registered = next_protocol_message(&mut socket).await?;
        assert!(matches!(
            registered,
            ServerMessage::StateChanged { state }
                if state.active_output_device_ids == ["living-room"]
                    && state.connected_devices.len() == 1
                    && state.connected_devices[0].name == "Living Room TV"
                    && state.connected_devices[0].is_output
        ));

        socket
            .send(TungsteniteMessage::Text(
                serde_json::json!({"type":"resume"}).to_string().into(),
            ))
            .await?;
        let mut saw_playing = false;
        for _ in 0..4 {
            if matches!(
                next_protocol_message(&mut socket).await?,
                ServerMessage::StateChanged { state } if state.is_playing
            ) {
                saw_playing = true;
                break;
            }
        }
        assert!(saw_playing);

        let logout = control_router
            .oneshot(
                Request::post("/api/auth/logout")
                    .header(COOKIE, format!("music_session={token}"))
                    .body(Body::empty())?,
            )
            .await?;
        assert_eq!(logout.status(), StatusCode::NO_CONTENT);
        let mut saw_revocation = false;
        let mut saw_guest_projection = false;
        for _ in 0..4 {
            match next_protocol_message(&mut socket).await? {
                ServerMessage::Error {
                    code: Some(ErrorCode::SessionRevoked),
                    ..
                } => saw_revocation = true,
                ServerMessage::StateChanged { state } if state.connected_devices.is_empty() => {
                    saw_guest_projection = true;
                }
                _ => {}
            }
            if saw_revocation && saw_guest_projection {
                break;
            }
        }
        assert!(saw_revocation);
        assert!(saw_guest_projection);

        socket
            .send(TungsteniteMessage::Text(
                serde_json::json!({"type":"pause"}).to_string().into(),
            ))
            .await?;
        let mut saw_guest_rejection = false;
        for _ in 0..3 {
            if matches!(
                next_protocol_message(&mut socket).await?,
                ServerMessage::Error { code: None, detail }
                    if detail.contains("guest sessions")
            ) {
                saw_guest_rejection = true;
                break;
            }
        }
        assert!(saw_guest_rejection);

        socket.close(None).await?;
        let _ = shutdown_tx.send(());
        server.await??;
        Ok(())
    }

    #[tokio::test]
    async fn catalog_scoped_actions_resolve_before_mutating_playback() -> Result<(), Box<dyn Error>>
    {
        let directory = tempdir()?;
        fs::create_dir_all(directory.path().join("music/Album/Sub"))?;
        fs::create_dir_all(directory.path().join("music/Else"))?;
        fs::write(
            directory.path().join("music/Album/01 - First.mp3"),
            b"catalog action fixture",
        )?;
        fs::write(
            directory.path().join("music/Album/Sub/02 - Second.mp3"),
            b"catalog action fixture",
        )?;
        fs::write(
            directory.path().join("music/Else/03 - Third.mp3"),
            b"catalog action fixture",
        )?;
        fs::create_dir_all(directory.path().join("modes/table/soundboards"))?;
        fs::create_dir_all(directory.path().join("modes/table/presets"))?;
        fs::write(
            directory.path().join("modes/table/manifest.yaml"),
            "id: table\nname: Table\npanels: [soundboard]\nplaylist_categories: []\ndefault_crossfade_ms: 0\ndefault_soundboard: main\n",
        )?;
        fs::write(
            directory.path().join("modes/table/soundboards/main.yaml"),
            "id: main\nname: Main\ncategories: []\n",
        )?;
        fs::write(
            directory.path().join("modes/table/presets/warm.yaml"),
            "id: warm\nname: Warm\neffects: []\ncrossfade_ms: 750\n",
        )?;

        let token = "catalog-actions-test-session-token";
        seed_session(directory.path(), token).await?;
        let runtime = AppRuntime::start(runtime_config(directory.path())?).await?;
        tokio::time::timeout(Duration::from_secs(5), async {
            loop {
                let status = runtime.library_status();
                if status.status == ReconciliationStatus::Current {
                    break;
                }
                assert_ne!(status.status, ReconciliationStatus::Failed);
                tokio::time::sleep(Duration::from_millis(10)).await;
            }
        })
        .await?;
        let router = runtime.router()?;
        let cookie = format!("music_session={token}");
        let search = router
            .oneshot(
                Request::get("/api/library/search?q=&sort=path&order=asc")
                    .header(COOKIE, &cookie)
                    .body(Body::empty())?,
            )
            .await?;
        assert_eq!(search.status(), StatusCode::OK);
        let search: serde_json::Value =
            serde_json::from_slice(&to_bytes(search.into_body(), 1024 * 1024).await?)?;
        let track_ids = search["tracks"]
            .as_array()
            .ok_or("library search tracks were missing")?
            .iter()
            .map(|track| track["id"].as_i64().ok_or("track id was missing"))
            .collect::<Result<Vec<_>, _>>()?;
        assert_eq!(track_ids.len(), 3);

        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await?;
        let address = listener.local_addr()?;
        let (shutdown_tx, shutdown_rx) = oneshot::channel();
        let server = tokio::spawn(runtime.run(listener, async move {
            let _ = shutdown_rx.await;
            Ok(())
        }));
        let mut request = format!("ws://{address}/api/ws").into_client_request()?;
        request
            .headers_mut()
            .insert(COOKIE, HeaderValue::from_str(&cookie)?);
        let (mut socket, _) = connect_async(request).await?;
        assert!(matches!(
            next_protocol_message(&mut socket).await?,
            ServerMessage::StateSnapshot { .. }
        ));

        socket
            .send(TungsteniteMessage::Text(
                serde_json::json!({
                    "type":"register",
                    "name":"Controller",
                    "client_id":"controller",
                    "protocol_version":2
                })
                .to_string()
                .into(),
            ))
            .await?;
        let _ = wait_for_protocol_message(&mut socket, "the registered controller", |message| {
            matches!(message, ServerMessage::StateChanged { state }
                if state.connected_devices.iter().any(|device| device.client_id == "controller"))
        })
        .await?;

        socket
            .send(TungsteniteMessage::Text(
                serde_json::json!({"type":"set_active_outputs","device_ids":["ghost"]})
                    .to_string()
                    .into(),
            ))
            .await?;
        assert!(matches!(
            wait_for_protocol_message(&mut socket, "the disconnected-output error", |message| {
                matches!(message, ServerMessage::Error { detail, .. }
                    if detail == "device(s) not connected: ghost")
            })
            .await?,
            ServerMessage::Error { .. }
        ));

        socket
            .send(TungsteniteMessage::Text(
                serde_json::json!({"type":"set_active_outputs","device_ids":["controller"]})
                    .to_string()
                    .into(),
            ))
            .await?;
        let _ = wait_for_protocol_message(&mut socket, "the active output", |message| {
            matches!(message, ServerMessage::StateChanged { state }
                if state.active_output_device_ids == ["controller"])
        })
        .await?;

        socket
            .send(TungsteniteMessage::Text(
                serde_json::json!({"type":"set_active_mode","mode_id":"missing"})
                    .to_string()
                    .into(),
            ))
            .await?;
        let _ = wait_for_protocol_message(&mut socket, "the unknown-mode error", |message| {
            matches!(message, ServerMessage::Error { detail, .. }
                if detail == "unknown mode: missing")
        })
        .await?;

        socket
            .send(TungsteniteMessage::Text(
                serde_json::json!({"type":"set_active_soundboard","soundboard_id":"main"})
                    .to_string()
                    .into(),
            ))
            .await?;
        let _ = wait_for_protocol_message(&mut socket, "the no-active-mode error", |message| {
            matches!(message, ServerMessage::Error { detail, .. }
                if detail == "no active mode; cannot set soundboard without a mode")
        })
        .await?;

        socket
            .send(TungsteniteMessage::Text(
                serde_json::json!({"type":"set_active_mode","mode_id":"table"})
                    .to_string()
                    .into(),
            ))
            .await?;
        let _ = wait_for_protocol_message(&mut socket, "the active mode", |message| {
            matches!(message, ServerMessage::StateChanged { state }
                if state.active_mode_id.as_deref() == Some("table"))
        })
        .await?;

        socket
            .send(TungsteniteMessage::Text(
                serde_json::json!({"type":"set_active_soundboard","soundboard_id":"main"})
                    .to_string()
                    .into(),
            ))
            .await?;
        let _ = wait_for_protocol_message(&mut socket, "the active soundboard", |message| {
            matches!(message, ServerMessage::StateChanged { state }
                if state.active_soundboard_id.as_deref() == Some("main"))
        })
        .await?;

        socket
            .send(TungsteniteMessage::Text(
                serde_json::json!({"type":"set_active_presets","preset_ids":["missing"]})
                    .to_string()
                    .into(),
            ))
            .await?;
        let _ = wait_for_protocol_message(&mut socket, "the unknown-preset error", |message| {
            matches!(message, ServerMessage::Error { detail, .. }
                if detail == "unknown preset(s): missing")
        })
        .await?;

        socket
            .send(TungsteniteMessage::Text(
                serde_json::json!({"type":"set_active_presets","preset_ids":["warm"]})
                    .to_string()
                    .into(),
            ))
            .await?;
        let _ = wait_for_protocol_message(&mut socket, "the effective preset", |message| {
            matches!(message, ServerMessage::StateChanged { state }
                if state.active_preset_ids == ["warm"] && state.crossfade_ms == 750)
        })
        .await?;

        socket
            .send(TungsteniteMessage::Text(
                serde_json::json!({"type":"ambient_play_track","track_id":i64::MAX})
                    .to_string()
                    .into(),
            ))
            .await?;
        let _ = wait_for_protocol_message(&mut socket, "the missing-track error", |message| {
            matches!(message, ServerMessage::Error { detail, .. }
                if detail == "track not in library")
        })
        .await?;

        socket
            .send(TungsteniteMessage::Text(
                serde_json::json!({"type":"ambient_play_track","track_id":track_ids[0]})
                    .to_string()
                    .into(),
            ))
            .await?;
        let _ = wait_for_protocol_message(&mut socket, "the direct ambient track", |message| {
            matches!(message, ServerMessage::StateChanged { state }
                if state.ambient.current_track_id == Some(track_ids[0]))
        })
        .await?;

        socket
            .send(TungsteniteMessage::Text(
                serde_json::json!({
                    "type":"ambient_set_queue",
                    "track_ids":[track_ids[2], i64::MAX]
                })
                .to_string()
                .into(),
            ))
            .await?;
        let _ = wait_for_protocol_message(&mut socket, "the invalid-queue error", |message| {
            matches!(message, ServerMessage::Error { detail, .. }
                if detail == "track not in library")
        })
        .await?;

        socket
            .send(TungsteniteMessage::Text(
                serde_json::json!({"type":"ambient_set_queue","track_ids":[track_ids[2]]})
                    .to_string()
                    .into(),
            ))
            .await?;
        let _ = wait_for_protocol_message(&mut socket, "the resolved queue", |message| {
            matches!(message, ServerMessage::StateChanged { state }
                if state.ambient.queue == [track_ids[2]])
        })
        .await?;

        socket
            .send(TungsteniteMessage::Text(
                serde_json::json!({
                    "type":"ambient_enqueue",
                    "track_id":track_ids[1],
                    "position":-1
                })
                .to_string()
                .into(),
            ))
            .await?;
        let _ = wait_for_protocol_message(&mut socket, "the clamped enqueue", |message| {
            matches!(message, ServerMessage::StateChanged { state }
                if state.ambient.queue == [track_ids[1], track_ids[2]])
        })
        .await?;

        socket
            .send(TungsteniteMessage::Text(
                serde_json::json!({
                    "type":"ambient_play_folder",
                    "path":"/Album/",
                    "start_index":1
                })
                .to_string()
                .into(),
            ))
            .await?;
        let _ = wait_for_protocol_message(&mut socket, "recursive folder playback", |message| {
            matches!(message, ServerMessage::StateChanged { state }
                if state.ambient.current_track_id == Some(track_ids[1])
                    && state.ambient.history == [track_ids[0]]
                    && state.ambient.queue.is_empty()
                    && state.ambient.source_playlist_id.is_none())
        })
        .await?;

        socket
            .send(TungsteniteMessage::Text(
                serde_json::json!({"type":"ambient_set_loop","loop":"follow"})
                    .to_string()
                    .into(),
            ))
            .await?;
        let _ = wait_for_protocol_message(&mut socket, "follow mode", |message| {
            matches!(message, ServerMessage::StateChanged { state }
                if state.ambient.loop_mode == music_protocol::LoopMode::Follow)
        })
        .await?;
        socket
            .send(TungsteniteMessage::Text(
                serde_json::json!({
                    "type":"ambient_skip_next",
                    "from_track_id":track_ids[1]
                })
                .to_string()
                .into(),
            ))
            .await?;
        let _ = wait_for_protocol_message(&mut socket, "the follow successor", |message| {
            matches!(message, ServerMessage::StateChanged { state }
                if state.ambient.current_track_id == Some(track_ids[2]))
        })
        .await?;

        socket
            .send(TungsteniteMessage::Text(
                serde_json::json!({
                    "type":"fire_interrupt_track",
                    "track_id":track_ids[0],
                    "return_to_ambient":true,
                    "fade_in_ms":25,
                    "fade_out_ms":50,
                    "duck_to":0.4
                })
                .to_string()
                .into(),
            ))
            .await?;
        let _ = wait_for_protocol_message(&mut socket, "the resolved interrupt", |message| {
            matches!(message, ServerMessage::StateChanged { state }
                if state.interrupt.as_ref().is_some_and(|interrupt|
                    interrupt.current_track_id == track_ids[0]
                        && interrupt.fade_in_ms == 25
                        && interrupt.fade_out_ms == 50
                        && interrupt.duck_to == Some(0.4)))
        })
        .await?;

        socket.close(None).await?;
        let _ = shutdown_tx.send(());
        server.await??;
        Ok(())
    }

    #[tokio::test]
    async fn soundboard_actions_validate_exact_items_and_catalog_updates_prune_loops()
    -> Result<(), Box<dyn Error>> {
        let directory = tempdir()?;
        fs::create_dir_all(directory.path().join("modes/table/soundboards"))?;
        fs::write(
            directory.path().join("modes/table/manifest.yaml"),
            "id: table\nname: Table\npanels: [soundboard]\nplaylist_categories: []\ndefault_crossfade_ms: 0\ndefault_soundboard: main\n",
        )?;
        fs::write(
            directory.path().join("modes/table/soundboards/main.yaml"),
            "id: main\nname: Main\ncategories:\n  - id: doors\n    name: Doors\n    items:\n      - file: dnd/door.ogg\n        name: Door\n",
        )?;
        let token = "soundboard-test-session-token";
        seed_session(directory.path(), token).await?;
        let runtime = AppRuntime::start(runtime_config(directory.path())?).await?;
        let control_router = runtime.router()?;
        let cookie = format!("music_session={token}");
        let active = control_router
            .clone()
            .oneshot(
                Request::put("/api/modes/active")
                    .header(COOKIE, &cookie)
                    .header("content-type", "application/json")
                    .body(Body::from(r#"{"mode_id":"table"}"#))?,
            )
            .await?;
        assert_eq!(active.status(), StatusCode::OK);

        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await?;
        let address = listener.local_addr()?;
        let (shutdown_tx, shutdown_rx) = oneshot::channel();
        let server = tokio::spawn(runtime.run(listener, async move {
            let _ = shutdown_rx.await;
            Ok(())
        }));
        let mut request = format!("ws://{address}/api/ws").into_client_request()?;
        request
            .headers_mut()
            .insert(COOKIE, HeaderValue::from_str(&cookie)?);
        let (mut socket, _) = connect_async(request).await?;
        assert!(matches!(
            next_protocol_message(&mut socket).await?,
            ServerMessage::StateSnapshot { state, .. }
                if state.active_mode_id.as_deref() == Some("table")
        ));

        socket
            .send(TungsteniteMessage::Text(
                serde_json::json!({
                    "type":"fire_sfx",
                    "soundboard_id":"main",
                    "item_path":"dnd/missing.ogg",
                    "volume":0.5
                })
                .to_string()
                .into(),
            ))
            .await?;
        let mut saw_item_rejection = false;
        for _ in 0..4 {
            if matches!(
                next_protocol_message(&mut socket).await.map_err(|error| format!("Soundboard item rejection: {error}"))?,
                ServerMessage::Error { detail, .. }
                    if detail == "item 'dnd/missing.ogg' not in soundboard 'main'"
            ) {
                saw_item_rejection = true;
                break;
            }
        }
        assert!(saw_item_rejection);

        socket
            .send(TungsteniteMessage::Text(
                serde_json::json!({
                    "type":"fire_sfx",
                    "soundboard_id":"main",
                    "item_path":"dnd/door.ogg",
                    "volume":0.5
                })
                .to_string()
                .into(),
            ))
            .await?;
        let mut saw_sfx = false;
        for _ in 0..4 {
            if matches!(
                next_protocol_message(&mut socket).await.map_err(|error| format!("Soundboard SFX event: {error}"))?,
                ServerMessage::SfxFired { soundboard_id, item_path, .. }
                    if soundboard_id == "main" && item_path == "dnd/door.ogg"
            ) {
                saw_sfx = true;
                break;
            }
        }
        assert!(saw_sfx);

        socket
            .send(TungsteniteMessage::Text(
                serde_json::json!({
                    "type":"start_loop",
                    "id":"rain-loop",
                    "name":"Rain",
                    "soundboard_id":"main",
                    "item_path":"dnd/door.ogg",
                    "interval_s":30.0,
                    "volume":0.4
                })
                .to_string()
                .into(),
            ))
            .await?;
        let mut saw_loop = false;
        for _ in 0..4 {
            if matches!(
                next_protocol_message(&mut socket).await.map_err(|error| format!("Soundboard loop start: {error}"))?,
                ServerMessage::StateChanged { state }
                    if state.looping_sfx.iter().any(|looping| looping.id == "rain-loop")
            ) {
                saw_loop = true;
                break;
            }
        }
        assert!(saw_loop);

        let deleted = control_router
            .oneshot(
                Request::delete("/api/modes/table/soundboards/main/categories/doors/items/0")
                    .header(COOKIE, &cookie)
                    .body(Body::empty())?,
            )
            .await?;
        assert_eq!(deleted.status(), StatusCode::OK);
        let mut saw_pruned_loop = false;
        for _ in 0..5 {
            if matches!(
                next_protocol_message(&mut socket).await.map_err(|error| format!("Soundboard loop pruning: {error}"))?,
                ServerMessage::StateChanged { state } if state.looping_sfx.is_empty()
            ) {
                saw_pruned_loop = true;
                break;
            }
        }
        assert!(saw_pruned_loop);

        socket.close(None).await?;
        let _ = shutdown_tx.send(());
        server.await??;
        Ok(())
    }

    #[tokio::test]
    async fn automatic_playlist_http_contract_resolves_into_playback_sequences()
    -> Result<(), Box<dyn Error>> {
        let directory = tempdir()?;
        fs::create_dir_all(directory.path().join("music/Album"))?;
        fs::write(
            directory.path().join("music/Album/01 - First.mp3"),
            b"playlist fixture",
        )?;
        fs::write(
            directory.path().join("music/Album/02 - Second.mp3"),
            b"playlist fixture",
        )?;
        let token = "playlist-test-session-token";
        seed_session(directory.path(), token).await?;
        let runtime = AppRuntime::start(runtime_config(directory.path())?).await?;
        tokio::time::timeout(Duration::from_secs(2), async {
            loop {
                let status = runtime.library_status();
                if status.status == ReconciliationStatus::Current {
                    break;
                }
                assert_ne!(status.status, ReconciliationStatus::Failed);
                tokio::time::sleep(Duration::from_millis(10)).await;
            }
        })
        .await?;
        let router = runtime.router()?;
        let cookie = format!("music_session={token}");

        let search = router
            .clone()
            .oneshot(
                Request::get("/api/library/search?q=&sort=path&order=asc")
                    .header(COOKIE, &cookie)
                    .body(Body::empty())?,
            )
            .await?;
        assert_eq!(search.status(), StatusCode::OK);
        let search: serde_json::Value =
            serde_json::from_slice(&to_bytes(search.into_body(), 1024 * 1024).await?)?;
        let track_ids = search["tracks"]
            .as_array()
            .ok_or("library search tracks were missing")?
            .iter()
            .map(|track| track["id"].as_i64().ok_or("track id was missing"))
            .collect::<Result<Vec<_>, _>>()?;
        assert_eq!(track_ids.len(), 2);

        let created = router
            .clone()
            .oneshot(
                Request::post("/api/playlists")
                    .header(COOKIE, &cookie)
                    .header("content-type", "application/json")
                    .body(Body::from(
                        r#"{"name":"Quiet scenes","category":"ambient"}"#,
                    ))?,
            )
            .await?;
        assert_eq!(created.status(), StatusCode::CREATED);
        let created: serde_json::Value =
            serde_json::from_slice(&to_bytes(created.into_body(), 1024 * 1024).await?)?;
        let playlist_id = created["id"].as_i64().ok_or("playlist id was missing")?;

        for track_id in &track_ids {
            let added = router
                .clone()
                .oneshot(
                    Request::post(format!("/api/playlists/{playlist_id}/tracks"))
                        .header(COOKIE, &cookie)
                        .header("content-type", "application/json")
                        .body(Body::from(format!(r#"{{"track_id":{track_id}}}"#)))?,
                )
                .await?;
            assert_eq!(added.status(), StatusCode::CREATED);
        }

        let rule = serde_json::json!({"schema":"automatic-playlist/v1"});
        let preview = router
            .clone()
            .oneshot(
                Request::post(format!("/api/playlists/{playlist_id}/automatic/preview"))
                    .header(COOKIE, &cookie)
                    .header("content-type", "application/json")
                    .body(Body::from(serde_json::json!({"rule":rule}).to_string()))?,
            )
            .await?;
        assert_eq!(preview.status(), StatusCode::OK);
        let preview: serde_json::Value =
            serde_json::from_slice(&to_bytes(preview.into_body(), 1024 * 1024).await?)?;
        assert_eq!(preview["library_tracks"], 2);
        assert_eq!(preview["matched_tracks"], 2);
        let signature = preview["source_signature"]
            .as_str()
            .ok_or("automatic preview signature was missing")?;
        let configured = router
            .clone()
            .oneshot(
                Request::put(format!("/api/playlists/{playlist_id}/automatic"))
                    .header(COOKIE, &cookie)
                    .header("content-type", "application/json")
                    .body(Body::from(
                        serde_json::json!({"rule":rule,"source_signature":signature}).to_string(),
                    ))?,
            )
            .await?;
        assert_eq!(configured.status(), StatusCode::OK);

        let managed = router
            .clone()
            .oneshot(
                Request::post(format!("/api/playlists/{playlist_id}/tracks"))
                    .header(COOKIE, &cookie)
                    .header("content-type", "application/json")
                    .body(Body::from(format!(r#"{{"track_id":{}}}"#, track_ids[0])))?,
            )
            .await?;
        assert_eq!(managed.status(), StatusCode::CONFLICT);
        let managed: serde_json::Value =
            serde_json::from_slice(&to_bytes(managed.into_body(), 1024 * 1024).await?)?;
        assert_eq!(
            managed["detail"]["code"],
            "automatic_playlist_items_managed"
        );

        let exported = router
            .clone()
            .oneshot(
                Request::get(format!("/api/playlists/{playlist_id}/export"))
                    .header(COOKIE, &cookie)
                    .body(Body::empty())?,
            )
            .await?;
        assert_eq!(exported.status(), StatusCode::OK);
        let exported =
            String::from_utf8(to_bytes(exported.into_body(), 1024 * 1024).await?.to_vec())?;
        assert!(exported.starts_with("#EXTM3U\n#PLAYLIST:Quiet scenes\n"));

        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await?;
        let address = listener.local_addr()?;
        let (shutdown_tx, shutdown_rx) = oneshot::channel();
        let server = tokio::spawn(runtime.run(listener, async move {
            let _ = shutdown_rx.await;
            Ok(())
        }));
        let mut request = format!("ws://{address}/api/ws").into_client_request()?;
        request
            .headers_mut()
            .insert(COOKIE, HeaderValue::from_str(&cookie)?);
        let (mut socket, _) = connect_async(request).await?;
        let _ = next_protocol_message(&mut socket).await?;
        socket
            .send(TungsteniteMessage::Text(
                serde_json::json!({
                    "type":"ambient_play_playlist",
                    "playlist_id":playlist_id,
                    "start_index":1
                })
                .to_string()
                .into(),
            ))
            .await?;
        let mut saw_playlist = false;
        for _ in 0..4 {
            if matches!(
                next_protocol_message(&mut socket).await?,
                ServerMessage::StateChanged { state }
                    if state.ambient.current_track_id == Some(track_ids[1])
                        && state.ambient.source_playlist_id == Some(playlist_id)
            ) {
                saw_playlist = true;
                break;
            }
        }
        assert!(saw_playlist);

        socket.close(None).await?;
        let _ = shutdown_tx.send(());
        server.await??;
        Ok(())
    }

    #[tokio::test]
    async fn cue_dispatch_commits_preset_playlist_position_loops_and_sfx_together()
    -> Result<(), Box<dyn Error>> {
        let directory = tempdir()?;
        fs::create_dir_all(directory.path().join("music/Arrival"))?;
        fs::write(
            directory.path().join("music/Arrival/01 - Approach.mp3"),
            b"cue fixture",
        )?;
        fs::write(
            directory.path().join("music/Arrival/02 - Reveal.mp3"),
            b"cue fixture",
        )?;
        fs::create_dir_all(directory.path().join("modes/table/soundboards"))?;
        fs::create_dir_all(directory.path().join("modes/table/presets"))?;
        fs::create_dir_all(directory.path().join("modes/table/cues"))?;
        fs::write(
            directory.path().join("modes/table/manifest.yaml"),
            "id: table\nname: Table\npanels: [soundboard, cues]\nplaylist_categories: [ambient]\ndefault_crossfade_ms: 0\ndefault_soundboard: main\n",
        )?;
        fs::write(
            directory.path().join("modes/table/soundboards/main.yaml"),
            "id: main\nname: Main\ncategories:\n  - id: scene\n    name: Scene\n    items:\n      - file: dnd/door.ogg\n        name: Door\n      - file: dnd/rain.ogg\n        name: Rain\n",
        )?;
        fs::write(
            directory.path().join("modes/table/presets/cinematic.yaml"),
            "id: cinematic\nname: Cinematic\neffects: []\ncrossfade_ms: 1500\n",
        )?;
        fs::write(
            directory.path().join("modes/table/cues/arrival.yaml"),
            "id: arrival\nname: Arrival\npreset: cinematic\nplaylist: Arrival music\nstart_index: 1\nstart_ms: 2500\nsfx:\n  - soundboard: main\n    item: dnd/door.ogg\n    volume: 0.7\nloops:\n  - soundboard: main\n    item: dnd/rain.ogg\n    interval_s: 30\n    volume: 0.4\n",
        )?;

        let token = "cue-test-session-token";
        seed_session(directory.path(), token).await?;
        let runtime = AppRuntime::start(runtime_config(directory.path())?).await?;
        tokio::time::timeout(Duration::from_secs(2), async {
            loop {
                let status = runtime.library_status();
                if status.status == ReconciliationStatus::Current {
                    break;
                }
                assert_ne!(status.status, ReconciliationStatus::Failed);
                tokio::time::sleep(Duration::from_millis(10)).await;
            }
        })
        .await?;
        let router = runtime.router()?;
        let cookie = format!("music_session={token}");
        let search = router
            .clone()
            .oneshot(
                Request::get("/api/library/search?q=&sort=path&order=asc")
                    .header(COOKIE, &cookie)
                    .body(Body::empty())?,
            )
            .await?;
        assert_eq!(search.status(), StatusCode::OK);
        let search: serde_json::Value =
            serde_json::from_slice(&to_bytes(search.into_body(), 1024 * 1024).await?)?;
        let track_ids = search["tracks"]
            .as_array()
            .ok_or("library search tracks were missing")?
            .iter()
            .map(|track| track["id"].as_i64().ok_or("track id was missing"))
            .collect::<Result<Vec<_>, _>>()?;
        assert_eq!(track_ids.len(), 2);

        let created = router
            .clone()
            .oneshot(
                Request::post("/api/playlists")
                    .header(COOKIE, &cookie)
                    .header("content-type", "application/json")
                    .body(Body::from(
                        r#"{"name":"Arrival music","mode_id":"table","category":"ambient"}"#,
                    ))?,
            )
            .await?;
        assert_eq!(created.status(), StatusCode::CREATED);
        let created: serde_json::Value =
            serde_json::from_slice(&to_bytes(created.into_body(), 1024 * 1024).await?)?;
        let playlist_id = created["id"].as_i64().ok_or("playlist id was missing")?;
        for track_id in &track_ids {
            let added = router
                .clone()
                .oneshot(
                    Request::post(format!("/api/playlists/{playlist_id}/tracks"))
                        .header(COOKIE, &cookie)
                        .header("content-type", "application/json")
                        .body(Body::from(format!(r#"{{"track_id":{track_id}}}"#)))?,
                )
                .await?;
            assert_eq!(added.status(), StatusCode::CREATED);
        }
        let active = router
            .oneshot(
                Request::put("/api/modes/active")
                    .header(COOKIE, &cookie)
                    .header("content-type", "application/json")
                    .body(Body::from(r#"{"mode_id":"table"}"#))?,
            )
            .await?;
        assert_eq!(active.status(), StatusCode::OK);

        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await?;
        let address = listener.local_addr()?;
        let (shutdown_tx, shutdown_rx) = oneshot::channel();
        let server = tokio::spawn(runtime.run(listener, async move {
            let _ = shutdown_rx.await;
            Ok(())
        }));
        let mut request = format!("ws://{address}/api/ws").into_client_request()?;
        request
            .headers_mut()
            .insert(COOKIE, HeaderValue::from_str(&cookie)?);
        let (mut socket, _) = connect_async(request).await?;
        assert!(matches!(
            next_protocol_message(&mut socket).await?,
            ServerMessage::StateSnapshot { state, .. }
                if state.active_mode_id.as_deref() == Some("table")
        ));

        socket
            .send(TungsteniteMessage::Text(
                serde_json::json!({"type":"fire_cue","cue_id":"missing"})
                    .to_string()
                    .into(),
            ))
            .await?;
        let mut saw_missing_cue = false;
        for _ in 0..2 {
            match next_protocol_message(&mut socket).await? {
                ServerMessage::StateChanged { .. } => {}
                ServerMessage::Error { detail, .. } => {
                    assert_eq!(detail, "unknown cue 'missing' in mode 'table'");
                    saw_missing_cue = true;
                    break;
                }
                message => {
                    return Err(io::Error::other(format!(
                        "unexpected missing-cue response: {message:?}"
                    ))
                    .into());
                }
            }
        }
        assert!(saw_missing_cue);

        socket
            .send(TungsteniteMessage::Text(
                serde_json::json!({"type":"fire_cue","cue_id":"arrival"})
                    .to_string()
                    .into(),
            ))
            .await?;
        let mut saw_atomic_state = false;
        let mut saw_sfx = false;
        for _ in 0..6 {
            match next_protocol_message(&mut socket).await? {
                ServerMessage::StateChanged { state }
                    if state.active_preset_ids == ["cinematic"]
                        && state.crossfade_ms == 1_500
                        && state.ambient.current_track_id == Some(track_ids[1])
                        && state.ambient.source_playlist_id == Some(playlist_id)
                        && state.ambient.position_ms >= 2_500
                        && state.looping_sfx.len() == 1
                        && state.looping_sfx[0].id == "cue:arrival:0"
                        && state.looping_sfx[0].name == "Rain" =>
                {
                    saw_atomic_state = true;
                }
                ServerMessage::SfxFired {
                    soundboard_id,
                    item_path,
                    volume,
                } if soundboard_id == "main"
                    && item_path == "dnd/door.ogg"
                    && (volume - 0.7).abs() < f64::EPSILON =>
                {
                    saw_sfx = true;
                }
                _ => {}
            }
            if saw_atomic_state && saw_sfx {
                break;
            }
        }
        assert!(saw_atomic_state);
        assert!(saw_sfx);

        socket.close(None).await?;
        let _ = shutdown_tx.send(());
        server.await??;
        Ok(())
    }
}
