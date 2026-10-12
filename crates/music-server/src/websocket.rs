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
    PlaybackActorError, PlaybackActorHandle, PlaybackPublication, ResolvedPlaybackCommand,
};
use music_domain::{
    CrossfadeType as DomainCrossfadeType, DomainEvent, InterruptFade, LoopMode as DomainLoopMode,
    LoopingSfx, PlaybackCommand, ShuffleMode as DomainShuffleMode, TrackId,
    UnitInterval as DomainUnitInterval,
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
#[path = "websocket_tests.rs"]
mod tests;
