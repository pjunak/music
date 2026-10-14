use std::collections::BTreeMap;
use std::error::Error;
use std::fmt::{self, Display, Formatter};
use std::future::Future;
use std::pin::Pin;

use music_domain::{
    AmbientState, CrossfadeType, LoopMode, PlaybackError, PlaybackState, PositionReport,
    ShuffleMode, TrackId, UnitInterval, materialize_positions,
};
use serde::{Deserialize, Serialize};

pub type StoreFuture<'a, T, E> = Pin<Box<dyn Future<Output = Result<T, E>> + Send + 'a>>;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StoredPlaybackSnapshot {
    pub state_json: String,
    pub storage_revision: i64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StoreCompareAndSwap {
    Updated { storage_revision: i64 },
    Conflict,
}

/// Persistence boundary for the single playback aggregate.
///
/// It deliberately exposes one compare-and-swap document instead of a
/// repository per field. The application owns serialization and the adapter
/// owns only storage mechanics.
pub trait PlaybackStateStore: Send + Sync + 'static {
    type Error: Error + Send + Sync + 'static;

    fn load(&self, id: i64) -> StoreFuture<'_, Option<StoredPlaybackSnapshot>, Self::Error>;

    fn insert_if_missing<'a>(
        &'a self,
        id: i64,
        state_json: &'a str,
    ) -> StoreFuture<'a, bool, Self::Error>;

    fn compare_and_swap<'a>(
        &'a self,
        id: i64,
        expected_storage_revision: i64,
        state_json: &'a str,
    ) -> StoreFuture<'a, StoreCompareAndSwap, Self::Error>;
}

#[derive(Debug)]
pub enum PersistedStateError {
    Json(serde_json::Error),
    Domain(PlaybackError),
}

impl Display for PersistedStateError {
    fn fmt(&self, formatter: &mut Formatter<'_>) -> fmt::Result {
        match self {
            Self::Json(_) => formatter.write_str("persisted playback state is not valid JSON"),
            Self::Domain(error) => {
                write!(formatter, "persisted playback state is invalid: {error}")
            }
        }
    }
}

impl Error for PersistedStateError {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        match self {
            Self::Json(source) => Some(source),
            Self::Domain(source) => Some(source),
        }
    }
}

impl From<serde_json::Error> for PersistedStateError {
    fn from(error: serde_json::Error) -> Self {
        Self::Json(error)
    }
}

impl From<PlaybackError> for PersistedStateError {
    fn from(error: PlaybackError) -> Self {
        Self::Domain(error)
    }
}

#[derive(Debug, Clone, Copy, Deserialize, Serialize)]
#[serde(rename_all = "snake_case")]
enum PersistedLoopMode {
    Off,
    Follow,
    Queue,
    Track,
}

#[derive(Debug, Clone, Copy, Deserialize, Serialize)]
#[serde(rename_all = "snake_case")]
enum PersistedShuffleMode {
    Off,
    Random,
}

#[derive(Debug, Clone, Copy, Deserialize, Serialize)]
#[serde(rename_all = "snake_case")]
enum PersistedCrossfadeType {
    Linear,
    EqualPower,
    Cut,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
struct PersistedAmbientState {
    current_track_id: Option<i64>,
    queue: Vec<i64>,
    history: Vec<i64>,
    position_ms: u64,
    loop_mode: PersistedLoopMode,
    shuffle: PersistedShuffleMode,
    source_playlist_id: Option<i64>,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
struct PersistedPositionReport {
    device_id: String,
    position_ms: u64,
    reported_at: f64,
}

/// The durable playback document. Interrupts and looping effects are
/// timer-backed session state, so they are never written; unknown fields
/// from older documents are ignored and dropped on the next write.
#[derive(Debug, Clone, Deserialize, Serialize)]
struct PersistedPlaybackState {
    revision: u64,
    position_epoch: u64,
    is_playing: bool,
    active_mode_id: Option<String>,
    active_output_device_ids: Vec<String>,
    default_device_volume: f64,
    device_volumes: BTreeMap<String, f64>,
    active_soundboard_id: Option<String>,
    active_preset_ids: Vec<String>,
    preset_revision: u64,
    crossfade_ms: u32,
    crossfade_type: PersistedCrossfadeType,
    ambient: PersistedAmbientState,
    last_position_report: Option<PersistedPositionReport>,
}

/// Decode the durable document. Session-only state is normalized away before
/// it can become live.
pub fn decode_persisted_state(state_json: &str) -> Result<PlaybackState, PersistedStateError> {
    let persisted: PersistedPlaybackState = serde_json::from_str(state_json)?;
    let mut state = PlaybackState {
        publication_revision: persisted.revision,
        position_epoch: persisted.position_epoch,
        is_playing: persisted.is_playing,
        active_mode_id: persisted.active_mode_id,
        active_output_device_ids: persisted.active_output_device_ids,
        default_device_volume: UnitInterval::new(persisted.default_device_volume)?,
        device_volumes: persisted
            .device_volumes
            .into_iter()
            .map(|(device_id, volume)| Ok((device_id, UnitInterval::new(volume)?)))
            .collect::<Result<_, PlaybackError>>()?,
        active_soundboard_id: persisted.active_soundboard_id,
        active_preset_ids: persisted.active_preset_ids,
        preset_revision: persisted.preset_revision,
        crossfade_ms: persisted.crossfade_ms,
        crossfade_type: persisted.crossfade_type.into(),
        ambient: persisted.ambient.try_into()?,
        // Interrupts and loops are timer-backed session state and can never
        // be resumed safely after process downtime.
        interrupt: None,
        looping_sfx: Vec::new(),
        last_position_report: persisted
            .last_position_report
            .filter(|report| report.reported_at.is_finite() && report.reported_at >= 0.0)
            .map(|report| PositionReport {
                device_id: report.device_id,
                position_ms: report.position_ms.min(i64::MAX as u64),
                reported_at_unix_seconds: report.reported_at,
            }),
    };
    state.normalize_for_startup();
    Ok(state)
}

/// Encode a materialized point-in-time snapshot. Monotonic anchors are never
/// durable because they are process-local and meaningless after restart.
pub fn encode_persisted_state(
    state: &PlaybackState,
    now_monotonic_ms: u64,
) -> Result<String, PersistedStateError> {
    let materialized = materialize_positions(state, now_monotonic_ms);
    Ok(serde_json::to_string(&PersistedPlaybackState::from(
        &materialized,
    ))?)
}

impl TryFrom<PersistedAmbientState> for AmbientState {
    type Error = PlaybackError;

    fn try_from(persisted: PersistedAmbientState) -> Result<Self, Self::Error> {
        Ok(Self {
            current_track_id: persisted.current_track_id.and_then(valid_track_id),
            queue: persisted
                .queue
                .into_iter()
                .filter_map(valid_track_id)
                .collect(),
            history: persisted
                .history
                .into_iter()
                .filter_map(valid_track_id)
                .collect(),
            position_ms: persisted.position_ms.min(i64::MAX as u64),
            position_anchor_ms: None,
            loop_mode: persisted.loop_mode.into(),
            shuffle: persisted.shuffle.into(),
            source_playlist_id: persisted.source_playlist_id,
        })
    }
}

impl From<&PlaybackState> for PersistedPlaybackState {
    fn from(state: &PlaybackState) -> Self {
        Self {
            revision: state.publication_revision,
            position_epoch: state.position_epoch,
            is_playing: state.is_playing,
            active_mode_id: state.active_mode_id.clone(),
            active_output_device_ids: state.active_output_device_ids.clone(),
            default_device_volume: state.default_device_volume.get(),
            device_volumes: state
                .device_volumes
                .iter()
                .map(|(device_id, volume)| (device_id.clone(), volume.get()))
                .collect(),
            active_soundboard_id: state.active_soundboard_id.clone(),
            active_preset_ids: state.active_preset_ids.clone(),
            preset_revision: state.preset_revision,
            crossfade_ms: state.crossfade_ms,
            crossfade_type: state.crossfade_type.into(),
            ambient: (&state.ambient).into(),
            last_position_report: state.last_position_report.as_ref().map(Into::into),
        }
    }
}

impl From<&AmbientState> for PersistedAmbientState {
    fn from(state: &AmbientState) -> Self {
        Self {
            current_track_id: state.current_track_id.map(TrackId::get),
            queue: state.queue.iter().copied().map(TrackId::get).collect(),
            history: state.history.iter().copied().map(TrackId::get).collect(),
            position_ms: state.position_ms,
            loop_mode: state.loop_mode.into(),
            shuffle: state.shuffle.into(),
            source_playlist_id: state.source_playlist_id,
        }
    }
}

impl From<&PositionReport> for PersistedPositionReport {
    fn from(state: &PositionReport) -> Self {
        Self {
            device_id: state.device_id.clone(),
            position_ms: state.position_ms,
            reported_at: state.reported_at_unix_seconds,
        }
    }
}

impl From<PersistedLoopMode> for LoopMode {
    fn from(value: PersistedLoopMode) -> Self {
        match value {
            PersistedLoopMode::Off => Self::Off,
            PersistedLoopMode::Follow => Self::Follow,
            PersistedLoopMode::Queue => Self::Queue,
            PersistedLoopMode::Track => Self::Track,
        }
    }
}

impl From<LoopMode> for PersistedLoopMode {
    fn from(value: LoopMode) -> Self {
        match value {
            LoopMode::Off => Self::Off,
            LoopMode::Follow => Self::Follow,
            LoopMode::Queue => Self::Queue,
            LoopMode::Track => Self::Track,
        }
    }
}

impl From<PersistedShuffleMode> for ShuffleMode {
    fn from(value: PersistedShuffleMode) -> Self {
        match value {
            PersistedShuffleMode::Off => Self::Off,
            PersistedShuffleMode::Random => Self::Random,
        }
    }
}

impl From<ShuffleMode> for PersistedShuffleMode {
    fn from(value: ShuffleMode) -> Self {
        match value {
            ShuffleMode::Off => Self::Off,
            ShuffleMode::Random => Self::Random,
        }
    }
}

impl From<PersistedCrossfadeType> for CrossfadeType {
    fn from(value: PersistedCrossfadeType) -> Self {
        match value {
            PersistedCrossfadeType::Linear => Self::Linear,
            PersistedCrossfadeType::EqualPower => Self::EqualPower,
            PersistedCrossfadeType::Cut => Self::Cut,
        }
    }
}

impl From<CrossfadeType> for PersistedCrossfadeType {
    fn from(value: CrossfadeType) -> Self {
        match value {
            CrossfadeType::Linear => Self::Linear,
            CrossfadeType::EqualPower => Self::EqualPower,
            CrossfadeType::Cut => Self::Cut,
        }
    }
}

fn valid_track_id(value: i64) -> Option<TrackId> {
    TrackId::new(value).ok()
}

#[cfg(test)]
mod tests {
    use std::error::Error;

    use music_domain::{
        ClockSample, PlaybackCommand, PlaybackState, ReductionContext, TrackId, UnitInterval,
        reduce,
    };
    use serde_json::{Value, json};

    use super::{decode_persisted_state, encode_persisted_state};

    fn playing_track(track_id: i64) -> Result<PlaybackState, Box<dyn Error>> {
        Ok(reduce(
            &PlaybackState::default(),
            PlaybackCommand::AmbientPlayTrack(TrackId::new(track_id)?),
            ReductionContext {
                clock: ClockSample::new(1_000, 1.0)?,
                random_queue_index: None,
            },
        )?
        .next_state)
    }

    #[test]
    fn canonical_encoding_materializes_position_and_omits_session_state()
    -> Result<(), Box<dyn Error>> {
        let encoded = encode_persisted_state(&playing_track(11)?, 2_500)?;
        let json: Value = serde_json::from_str(&encoded)?;
        assert_eq!(json["ambient"]["position_ms"], 1_500);
        assert_eq!(json["default_device_volume"], 1.0);
        for retired in ["volume", "interrupt", "looping_sfx"] {
            assert!(json.get(retired).is_none(), "{retired} is still written");
        }
        assert!(json["ambient"].get("position_anchored_at").is_none());

        let restarted = decode_persisted_state(&encoded)?;
        assert_eq!(restarted.ambient.current_track_id, Some(TrackId::new(11)?));
        assert_eq!(restarted.ambient.position_ms, 1_500);
        assert!(!restarted.is_playing);
        assert_eq!(restarted.ambient.position_anchor_ms, None);
        Ok(())
    }

    #[test]
    fn documents_with_retired_fields_decode_and_shed_them_on_the_next_write()
    -> Result<(), Box<dyn Error>> {
        let mut document: Value =
            serde_json::from_str(&encode_persisted_state(&playing_track(11)?, 1_000)?)?;
        document["volume"] = json!(0.5);
        document["device_volumes"] = json!({ "tv": 0.25 });
        document["active_output_device_ids"] = json!(["tv"]);
        document["interrupt"] = json!({ "current_track_id": 12, "queue": [] });
        document["looping_sfx"] = json!([{ "id": "rain" }]);
        document["ambient"]["position_anchored_at"] = Value::Null;

        let state = decode_persisted_state(&document.to_string())?;
        assert_eq!(state.default_device_volume, UnitInterval::new(1.0)?);
        assert_eq!(state.device_volumes["tv"], UnitInterval::new(0.25)?);
        assert!(state.active_output_device_ids.is_empty());
        assert!(state.interrupt.is_none());
        assert!(state.looping_sfx.is_empty());

        let rewritten: Value = serde_json::from_str(&encode_persisted_state(&state, 1_000)?)?;
        assert!(rewritten.get("volume").is_none());
        assert!(rewritten.get("interrupt").is_none());
        Ok(())
    }

    #[test]
    fn documents_without_absolute_device_volume_are_rejected() {
        assert!(decode_persisted_state(r#"{"revision":7,"is_playing":false}"#).is_err());
    }
}
