use serde::{Deserialize, Serialize};
use ts_rs::TS;

use crate::actions::{CrossfadeType, LoopMode, ShuffleMode};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
pub struct DeviceInfo {
    pub device_id: String,
    pub client_id: String,
    pub name: String,
    #[serde(default)]
    pub is_output: bool,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
pub struct PositionReport {
    pub device_id: String,
    pub position_ms: i64,
    pub reported_at: f64,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default, TS)]
#[serde(default)]
#[ts(rename = "CanonicalAmbientState")]
pub struct AmbientState {
    pub current_track_id: Option<i64>,
    pub queue: Vec<i64>,
    pub history: Vec<i64>,
    pub position_ms: i64,
    pub position_anchored_at: Option<f64>,
    #[serde(rename = "loop")]
    pub loop_mode: LoopMode,
    pub shuffle: ShuffleMode,
    pub source_playlist_id: Option<i64>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[ts(rename = "CanonicalInterruptState")]
pub struct InterruptState {
    pub current_track_id: i64,
    #[serde(default)]
    pub queue: Vec<i64>,
    #[serde(default)]
    pub position_ms: i64,
    pub position_anchored_at: Option<f64>,
    #[serde(default = "default_true")]
    pub return_to_ambient: bool,
    #[serde(default)]
    pub fade_in_ms: i64,
    #[serde(default)]
    pub fade_out_ms: i64,
    pub duck_to: Option<f64>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
pub struct LoopingSfx {
    pub id: String,
    pub name: String,
    pub soundboard_id: String,
    pub item_path: String,
    pub interval_s: f64,
    #[serde(default = "default_one")]
    pub volume: f64,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[serde(default)]
#[ts(rename = "CanonicalPlayerState")]
pub struct PlayerState {
    pub revision: i64,
    pub position_epoch: i64,
    pub is_playing: bool,
    pub volume: f64,
    pub active_mode_id: Option<String>,
    pub active_output_device_ids: Vec<String>,
    pub default_device_volume: f64,
    pub device_volumes: std::collections::BTreeMap<String, f64>,
    pub active_soundboard_id: Option<String>,
    pub active_preset_ids: Vec<String>,
    pub preset_revision: i64,
    pub crossfade_ms: i64,
    pub crossfade_type: CrossfadeType,
    pub ambient: AmbientState,
    pub interrupt: Option<InterruptState>,
    pub looping_sfx: Vec<LoopingSfx>,
    pub last_position_report: Option<PositionReport>,
    pub connected_devices: Vec<DeviceInfo>,
}

impl Default for PlayerState {
    fn default() -> Self {
        Self {
            revision: 0,
            position_epoch: 0,
            is_playing: false,
            volume: 1.0,
            active_mode_id: None,
            active_output_device_ids: Vec::new(),
            default_device_volume: 1.0,
            device_volumes: std::collections::BTreeMap::new(),
            active_soundboard_id: None,
            active_preset_ids: Vec::new(),
            preset_revision: 0,
            crossfade_ms: 0,
            crossfade_type: CrossfadeType::Linear,
            ambient: AmbientState::default(),
            interrupt: None,
            looping_sfx: Vec::new(),
            last_position_report: None,
            connected_devices: Vec::new(),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "snake_case")]
pub enum ErrorCode {
    SessionExpired,
    SessionRevoked,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[serde(tag = "type", rename_all = "snake_case")]
#[ts(rename = "CanonicalServerMessage")]
pub enum ServerMessage {
    StateSnapshot {
        #[serde(default)]
        your_device_id: String,
        state: PlayerState,
    },
    StateChanged {
        state: PlayerState,
    },
    SfxFired {
        soundboard_id: String,
        item_path: String,
        #[serde(default = "default_one")]
        volume: f64,
    },
    Error {
        detail: String,
        code: Option<ErrorCode>,
    },
}

impl ServerMessage {
    #[must_use]
    pub const fn kind(&self) -> &'static str {
        match self {
            Self::StateSnapshot { .. } => "state_snapshot",
            Self::StateChanged { .. } => "state_changed",
            Self::SfxFired { .. } => "sfx_fired",
            Self::Error { .. } => "error",
        }
    }
}

const fn default_true() -> bool {
    true
}

const fn default_one() -> f64 {
    1.0
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeSet;
    use std::error::Error;
    use std::io;

    use serde::Deserialize;
    use serde_json::Value;

    use super::ServerMessage;

    const CORPUS: &str = include_str!(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../../contracts/reference/v1/websocket-messages.examples.json"
    ));

    #[derive(Deserialize)]
    struct Corpus {
        invalid: Vec<InvalidCase>,
        message_types: Vec<String>,
        valid: Vec<ValidCase>,
    }

    #[derive(Deserialize)]
    struct InvalidCase {
        id: String,
        input: Value,
    }

    #[derive(Deserialize)]
    struct ValidCase {
        canonical: Value,
        id: String,
        input: Value,
    }

    #[test]
    fn python_message_corpus_round_trips_canonically() -> Result<(), Box<dyn Error>> {
        let corpus: Corpus = serde_json::from_str(CORPUS)?;
        let mut kinds = BTreeSet::new();

        for case in corpus.valid {
            let message: ServerMessage = serde_json::from_value(case.input).map_err(|error| {
                io::Error::new(io::ErrorKind::InvalidData, format!("{}: {error}", case.id))
            })?;
            kinds.insert(message.kind().to_owned());
            assert_eq!(
                serde_json::to_value(message)?,
                case.canonical,
                "{}",
                case.id
            );
        }

        assert_eq!(
            kinds,
            corpus.message_types.into_iter().collect::<BTreeSet<_>>()
        );
        Ok(())
    }

    #[test]
    fn python_message_rejection_corpus_is_rejected() -> Result<(), Box<dyn Error>> {
        let corpus: Corpus = serde_json::from_str(CORPUS)?;
        for case in corpus.invalid {
            assert!(
                serde_json::from_value::<ServerMessage>(case.input).is_err(),
                "Rust accepted Python-rejected case {}",
                case.id
            );
        }
        Ok(())
    }
}
