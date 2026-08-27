use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::scalar::{
    BoundedText, CrossfadeMillis, FadeMillis, LoopIntervalSeconds, NonNegativeI64, ProtocolVersion,
    UnitInterval,
};

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum LoopMode {
    #[default]
    Off,
    Follow,
    Queue,
    Track,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ShuffleMode {
    #[default]
    Off,
    Random,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CrossfadeType {
    #[default]
    Linear,
    EqualPower,
    Cut,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum ClientAction {
    Register {
        name: BoundedText<1, 128>,
        client_id: BoundedText<1, 64>,
        #[serde(default)]
        protocol_version: ProtocolVersion,
    },
    SetVolume {
        volume: UnitInterval,
    },
    Pause,
    Resume,
    SetActiveMode {
        #[serde(deserialize_with = "crate::scalar::required_nullable")]
        mode_id: Option<String>,
    },
    SetActiveOutputs {
        #[serde(default)]
        device_ids: Vec<String>,
    },
    SetDeviceVolume {
        device_id: BoundedText<1, 64>,
        volume: UnitInterval,
    },
    PositionReport {
        position_ms: NonNegativeI64,
    },
    AmbientPlayTrack {
        track_id: i64,
    },
    AmbientSetQueue {
        #[serde(default)]
        track_ids: Vec<i64>,
    },
    AmbientJumpQueue {
        position: NonNegativeI64,
    },
    AmbientEnqueue {
        track_id: i64,
        position: Option<i64>,
    },
    AmbientClearQueue,
    AmbientSkipNext {
        from_track_id: Option<i64>,
    },
    AmbientSkipPrev,
    AmbientSeek {
        position_ms: NonNegativeI64,
    },
    AmbientSetLoop {
        #[serde(rename = "loop")]
        loop_mode: LoopMode,
    },
    AmbientSetShuffle {
        shuffle: ShuffleMode,
    },
    AmbientStop,
    AmbientPlayPlaylist {
        playlist_id: i64,
        #[serde(default)]
        start_index: NonNegativeI64,
    },
    AmbientPlayFolder {
        #[serde(default)]
        path: BoundedText<0, 1024>,
        #[serde(default)]
        start_index: NonNegativeI64,
    },
    SetActiveSoundboard {
        #[serde(deserialize_with = "crate::scalar::required_nullable")]
        soundboard_id: Option<String>,
    },
    SetActivePresets {
        #[serde(default)]
        preset_ids: Vec<String>,
    },
    SetCrossfade {
        crossfade_ms: CrossfadeMillis,
        crossfade_type: Option<CrossfadeType>,
    },
    FireInterruptTrack {
        duck_to: Option<UnitInterval>,
        track_id: i64,
        #[serde(default = "default_true")]
        return_to_ambient: bool,
        #[serde(default)]
        fade_in_ms: FadeMillis,
        #[serde(default)]
        fade_out_ms: FadeMillis,
    },
    FireInterruptPlaylist {
        playlist_id: i64,
        #[serde(default = "default_true")]
        return_to_ambient: bool,
        #[serde(default)]
        fade_in_ms: FadeMillis,
        #[serde(default)]
        fade_out_ms: FadeMillis,
        duck_to: Option<UnitInterval>,
    },
    InterruptSkipNext {
        from_track_id: Option<i64>,
    },
    InterruptSeek {
        position_ms: NonNegativeI64,
    },
    CancelInterrupt,
    FireSfx {
        soundboard_id: BoundedText<1, 128>,
        item_path: BoundedText<1, 512>,
        #[serde(default)]
        volume: UnitInterval,
    },
    StartLoop {
        id: BoundedText<1, 64>,
        name: BoundedText<1, 128>,
        soundboard_id: BoundedText<1, 128>,
        item_path: BoundedText<1, 512>,
        interval_s: LoopIntervalSeconds,
        #[serde(default)]
        volume: UnitInterval,
    },
    StopLoop {
        id: BoundedText<1, 64>,
    },
    FireCue {
        cue_id: BoundedText<1, 128>,
    },
}

const fn default_true() -> bool {
    true
}

impl ClientAction {
    pub fn from_value(value: Value) -> Result<Self, serde_json::Error> {
        serde_json::from_value(value)
    }

    #[must_use]
    pub const fn kind(&self) -> &'static str {
        match self {
            Self::Register { .. } => "register",
            Self::SetVolume { .. } => "set_volume",
            Self::Pause => "pause",
            Self::Resume => "resume",
            Self::SetActiveMode { .. } => "set_active_mode",
            Self::SetActiveOutputs { .. } => "set_active_outputs",
            Self::SetDeviceVolume { .. } => "set_device_volume",
            Self::PositionReport { .. } => "position_report",
            Self::AmbientPlayTrack { .. } => "ambient_play_track",
            Self::AmbientSetQueue { .. } => "ambient_set_queue",
            Self::AmbientJumpQueue { .. } => "ambient_jump_queue",
            Self::AmbientEnqueue { .. } => "ambient_enqueue",
            Self::AmbientClearQueue => "ambient_clear_queue",
            Self::AmbientSkipNext { .. } => "ambient_skip_next",
            Self::AmbientSkipPrev => "ambient_skip_prev",
            Self::AmbientSeek { .. } => "ambient_seek",
            Self::AmbientSetLoop { .. } => "ambient_set_loop",
            Self::AmbientSetShuffle { .. } => "ambient_set_shuffle",
            Self::AmbientStop => "ambient_stop",
            Self::AmbientPlayPlaylist { .. } => "ambient_play_playlist",
            Self::AmbientPlayFolder { .. } => "ambient_play_folder",
            Self::SetActiveSoundboard { .. } => "set_active_soundboard",
            Self::SetActivePresets { .. } => "set_active_presets",
            Self::SetCrossfade { .. } => "set_crossfade",
            Self::FireInterruptTrack { .. } => "fire_interrupt_track",
            Self::FireInterruptPlaylist { .. } => "fire_interrupt_playlist",
            Self::InterruptSkipNext { .. } => "interrupt_skip_next",
            Self::InterruptSeek { .. } => "interrupt_seek",
            Self::CancelInterrupt => "cancel_interrupt",
            Self::FireSfx { .. } => "fire_sfx",
            Self::StartLoop { .. } => "start_loop",
            Self::StopLoop { .. } => "stop_loop",
            Self::FireCue { .. } => "fire_cue",
        }
    }
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeSet;
    use std::error::Error;
    use std::io;

    use serde::Deserialize;
    use serde_json::Value;

    use super::ClientAction;

    const CORPUS: &str = include_str!(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../../contracts/reference/v1/websocket-actions.examples.json"
    ));

    #[derive(Deserialize)]
    struct Corpus {
        action_types: Vec<String>,
        invalid: Vec<InvalidCase>,
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
    fn python_action_corpus_round_trips_canonically() -> Result<(), Box<dyn Error>> {
        let corpus: Corpus = serde_json::from_str(CORPUS)?;
        let mut kinds = BTreeSet::new();

        for case in corpus.valid {
            let action = ClientAction::from_value(case.input).map_err(|error| {
                io::Error::new(io::ErrorKind::InvalidData, format!("{}: {error}", case.id))
            })?;
            kinds.insert(action.kind().to_owned());
            assert_eq!(serde_json::to_value(action)?, case.canonical, "{}", case.id);
        }

        assert_eq!(
            kinds,
            corpus.action_types.into_iter().collect::<BTreeSet<_>>()
        );
        Ok(())
    }

    #[test]
    fn python_rejection_corpus_is_rejected() -> Result<(), Box<dyn Error>> {
        let corpus: Corpus = serde_json::from_str(CORPUS)?;
        for case in corpus.invalid {
            assert!(
                ClientAction::from_value(case.input).is_err(),
                "Rust accepted Python-rejected case {}",
                case.id
            );
        }
        Ok(())
    }
}
