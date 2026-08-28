use std::fmt::{self, Display, Formatter};
use std::sync::Arc;
use std::time::Duration;

use futures_util::{SinkExt, StreamExt};
use music_protocol::{BoundedText, ClientAction, NonNegativeI64, ProtocolVersion, ServerMessage};
use serde_json::Value;
use tokio::sync::watch;
use tokio::time::{Instant, MissedTickBehavior};
use tokio_tungstenite::tungstenite::protocol::WebSocketConfig;
use tokio_tungstenite::tungstenite::{Error as WebSocketError, Message};

use crate::mpv::MpvError;
use crate::reconcile::VolumeContract;
use crate::runtime::OutputRuntime;

const RECONNECT_DELAY: Duration = Duration::from_secs(3);
const PING_INTERVAL: Duration = Duration::from_secs(20);
const PING_TIMEOUT: Duration = Duration::from_secs(10);
const POSITION_REPORT_INTERVAL: Duration = Duration::from_secs(1);
const AUDIO_HEALTH_INTERVAL: Duration = Duration::from_secs(5);
const MAX_WEBSOCKET_MESSAGE_BYTES: usize = 2 * 1024 * 1024;
const MAX_SFX_PATH_CHARS: usize = 512;

#[derive(Debug)]
pub struct OutputClientError(String);

impl Display for OutputClientError {
    fn fmt(&self, formatter: &mut Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.0)
    }
}

impl std::error::Error for OutputClientError {}

#[derive(Debug)]
enum ConnectionError {
    Retryable(String),
    Audio(MpvError),
    Fatal(String),
}

pub async fn run_websocket_client(
    runtime: Arc<OutputRuntime>,
    mut shutdown: watch::Receiver<bool>,
) -> Result<(), OutputClientError> {
    loop {
        if *shutdown.borrow() {
            return Ok(());
        }
        match run_connection(Arc::clone(&runtime), shutdown.clone()).await {
            Ok(()) => return Ok(()),
            Err(ConnectionError::Audio(error)) => {
                return Err(OutputClientError(format!("audio backend failed: {error}")));
            }
            Err(ConnectionError::Fatal(error)) => return Err(OutputClientError(error)),
            Err(ConnectionError::Retryable(error)) => {
                tracing::warn!(error = %error, "WebSocket connection ended; reconnecting");
            }
        }
        tokio::select! {
            () = tokio::time::sleep(RECONNECT_DELAY) => {}
            changed = shutdown.changed() => {
                if changed.is_err() || *shutdown.borrow() {
                    return Ok(());
                }
            }
        }
    }
}

async fn run_connection(
    runtime: Arc<OutputRuntime>,
    mut shutdown: watch::Receiver<bool>,
) -> Result<(), ConnectionError> {
    let websocket_url = runtime.config().websocket_url().to_string();
    let websocket_config = WebSocketConfig::default()
        .max_message_size(Some(MAX_WEBSOCKET_MESSAGE_BYTES))
        .max_frame_size(Some(MAX_WEBSOCKET_MESSAGE_BYTES));
    let (socket, _) = tokio_tungstenite::connect_async_with_config(
        websocket_url.as_str(),
        Some(websocket_config),
        false,
    )
    .await
    .map_err(retryable_websocket)?;
    runtime.begin_connection().await;
    let (mut writer, mut reader) = socket.split();
    send_action(
        &mut writer,
        ClientAction::Register {
            name: BoundedText::new(runtime.config().name.clone())
                .map_err(|error| ConnectionError::Fatal(error.to_string()))?,
            client_id: BoundedText::new(runtime.config().client_id.clone())
                .map_err(|error| ConnectionError::Fatal(error.to_string()))?,
            protocol_version: ProtocolVersion::new(2)
                .map_err(|error| ConnectionError::Fatal(error.to_string()))?,
        },
    )
    .await?;
    tracing::info!(
        url = %websocket_url,
        name = %runtime.config().name,
        client_id = %runtime.config().client_id,
        "connected and registered output"
    );

    let mut ping = tokio::time::interval(PING_INTERVAL);
    ping.set_missed_tick_behavior(MissedTickBehavior::Delay);
    ping.tick().await;
    let mut watchdog = tokio::time::interval(Duration::from_secs(1));
    watchdog.set_missed_tick_behavior(MissedTickBehavior::Delay);
    let mut reports = tokio::time::interval(POSITION_REPORT_INTERVAL);
    reports.set_missed_tick_behavior(MissedTickBehavior::Skip);
    reports.tick().await;
    let mut audio_health = tokio::time::interval(AUDIO_HEALTH_INTERVAL);
    audio_health.set_missed_tick_behavior(MissedTickBehavior::Delay);
    audio_health.tick().await;
    let mut ping_sent_at: Option<Instant> = None;

    loop {
        tokio::select! {
            changed = shutdown.changed() => {
                if changed.is_err() || *shutdown.borrow() {
                    let _ = writer.send(Message::Close(None)).await;
                    return Ok(());
                }
            }
            _ = ping.tick() => {
                writer
                    .send(Message::Ping(Vec::new().into()))
                    .await
                    .map_err(retryable_websocket)?;
                ping_sent_at = Some(Instant::now());
            }
            _ = watchdog.tick() => {
                if ping_sent_at.is_some_and(|sent| sent.elapsed() >= PING_TIMEOUT) {
                    return Err(ConnectionError::Retryable("WebSocket ping timed out".to_owned()));
                }
            }
            _ = reports.tick() => {
                if let Some(position_ms) = runtime
                    .position_report_millis()
                    .await
                    .map_err(ConnectionError::Audio)?
                {
                    let position_ms = NonNegativeI64::new(position_ms)
                        .map_err(|error| ConnectionError::Fatal(error.to_string()))?;
                    send_action(&mut writer, ClientAction::PositionReport { position_ms }).await?;
                }
            }
            _ = audio_health.tick() => {
                runtime.audio_healthcheck().await.map_err(ConnectionError::Audio)?;
            }
            message = reader.next() => {
                let Some(message) = message else {
                    return Err(ConnectionError::Retryable("WebSocket peer closed the stream".to_owned()));
                };
                let message = message.map_err(retryable_websocket)?;
                ping_sent_at = None;
                match message {
                    Message::Text(text) => handle_message(&runtime, text.as_bytes()).await?,
                    Message::Binary(bytes) => handle_message(&runtime, &bytes).await?,
                    Message::Ping(payload) => {
                        writer.send(Message::Pong(payload)).await.map_err(retryable_websocket)?;
                    }
                    Message::Pong(_) => {}
                    Message::Close(frame) => {
                        return Err(ConnectionError::Retryable(format!("WebSocket closed: {frame:?}")));
                    }
                    Message::Frame(_) => {}
                }
            }
        }
    }
}

async fn send_action<S>(writer: &mut S, action: ClientAction) -> Result<(), ConnectionError>
where
    S: futures_util::Sink<Message, Error = WebSocketError> + Unpin,
{
    let encoded = serde_json::to_string(&action)
        .map_err(|error| ConnectionError::Fatal(format!("could not encode action: {error}")))?;
    writer
        .send(Message::Text(encoded.into()))
        .await
        .map_err(retryable_websocket)
}

async fn handle_message(runtime: &OutputRuntime, encoded: &[u8]) -> Result<(), ConnectionError> {
    if encoded.len() > MAX_WEBSOCKET_MESSAGE_BYTES {
        return Err(ConnectionError::Retryable(
            "WebSocket message exceeded the size limit".to_owned(),
        ));
    }
    let raw: Value = match serde_json::from_slice(encoded) {
        Ok(raw) => raw,
        Err(error) => {
            tracing::warn!(error = %error, "ignored invalid WebSocket JSON");
            return Ok(());
        }
    };
    let volume_contract =
        raw.get("state")
            .and_then(Value::as_object)
            .map_or(VolumeContract::CanonicalV2, |state| {
                if state.contains_key("default_device_volume") {
                    VolumeContract::CanonicalV2
                } else {
                    VolumeContract::LegacyMasterTimesTrim
                }
            });
    let message: ServerMessage = match serde_json::from_value(raw) {
        Ok(message) => message,
        Err(error) => {
            tracing::warn!(error = %error, "ignored invalid server message");
            return Ok(());
        }
    };
    match message {
        ServerMessage::StateSnapshot { state, .. } | ServerMessage::StateChanged { state } => {
            let _ = runtime
                .apply_state(state, volume_contract)
                .await
                .map_err(ConnectionError::Audio)?;
        }
        ServerMessage::SfxFired {
            item_path, volume, ..
        } => {
            if item_path.chars().count() <= MAX_SFX_PATH_CHARS {
                runtime
                    .fire_sfx(&item_path, volume)
                    .await
                    .map_err(ConnectionError::Audio)?;
            }
        }
        ServerMessage::Error { detail, code } => {
            tracing::warn!(detail = %detail, code = ?code, "server rejected an output action");
        }
    }
    Ok(())
}

fn retryable_websocket(error: WebSocketError) -> ConnectionError {
    ConnectionError::Retryable(error.to_string())
}

#[cfg(test)]
mod tests {
    use music_protocol::AmbientState;

    use super::*;

    #[test]
    fn detects_legacy_volume_by_wire_field_presence() -> Result<(), Box<dyn std::error::Error>> {
        let state = music_protocol::PlayerState {
            ambient: AmbientState {
                current_track_id: Some(1),
                ..AmbientState::default()
            },
            ..music_protocol::PlayerState::default()
        };
        let mut value = serde_json::to_value(ServerMessage::StateChanged { state })?;
        value
            .get_mut("state")
            .and_then(Value::as_object_mut)
            .ok_or("missing state")?
            .remove("default_device_volume");
        let object = value
            .get("state")
            .and_then(Value::as_object)
            .ok_or("missing state")?;
        assert!(!object.contains_key("default_device_volume"));
        assert!(serde_json::from_value::<ServerMessage>(value).is_ok());
        Ok(())
    }
}
