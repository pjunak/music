use serde::{Deserialize, Serialize};
use ts_rs::TS;
use utoipa::ToSchema;
use utoipa::openapi::schema::{AnyOfBuilder, ArrayBuilder, ObjectBuilder, Schema, Type};
use utoipa::openapi::{Ref, RefOr};

use crate::actions::{CrossfadeType, LoopMode, ShuffleMode};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS, ToSchema)]
pub struct DeviceInfo {
    pub device_id: String,
    pub client_id: String,
    pub name: String,
    #[serde(default)]
    #[schema(schema_with = boolean_default_false_schema)]
    pub is_output: bool,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS, ToSchema)]
pub struct PositionReport {
    pub device_id: String,
    #[schema(schema_with = integer_schema)]
    pub position_ms: i64,
    #[schema(schema_with = number_schema)]
    pub reported_at: f64,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default, TS, ToSchema)]
#[serde(default)]
#[ts(rename = "CanonicalAmbientState")]
pub struct AmbientState {
    #[schema(schema_with = nullable_integer_schema)]
    pub current_track_id: Option<i64>,
    #[schema(schema_with = integer_array_schema)]
    pub queue: Vec<i64>,
    #[schema(schema_with = integer_array_schema)]
    pub history: Vec<i64>,
    #[schema(schema_with = integer_default_zero_schema)]
    pub position_ms: i64,
    #[schema(schema_with = nullable_number_schema)]
    pub position_anchored_at: Option<f64>,
    #[serde(rename = "loop")]
    #[schema(schema_with = loop_mode_schema)]
    pub loop_mode: LoopMode,
    #[schema(schema_with = shuffle_mode_schema)]
    pub shuffle: ShuffleMode,
    #[schema(schema_with = nullable_integer_schema)]
    pub source_playlist_id: Option<i64>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS, ToSchema)]
#[ts(rename = "CanonicalInterruptState")]
pub struct InterruptState {
    #[schema(schema_with = integer_schema)]
    pub current_track_id: i64,
    #[serde(default)]
    #[schema(schema_with = integer_array_schema)]
    pub queue: Vec<i64>,
    #[serde(default)]
    #[schema(schema_with = integer_default_zero_schema)]
    pub position_ms: i64,
    #[schema(schema_with = nullable_number_schema)]
    pub position_anchored_at: Option<f64>,
    #[serde(default = "default_true")]
    #[schema(schema_with = boolean_default_true_schema)]
    pub return_to_ambient: bool,
    #[serde(default)]
    #[schema(schema_with = integer_default_zero_schema)]
    pub fade_in_ms: i64,
    #[serde(default)]
    #[schema(schema_with = integer_default_zero_schema)]
    pub fade_out_ms: i64,
    #[schema(schema_with = nullable_unit_number_schema)]
    pub duck_to: Option<f64>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS, ToSchema)]
pub struct LoopingSfx {
    pub id: String,
    pub name: String,
    pub soundboard_id: String,
    pub item_path: String,
    #[schema(schema_with = loop_interval_schema)]
    pub interval_s: f64,
    #[serde(default = "default_one")]
    #[schema(schema_with = unit_number_default_one_schema)]
    pub volume: f64,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS, ToSchema)]
#[serde(default)]
#[ts(rename = "CanonicalPlayerState")]
pub struct PlayerState {
    #[schema(schema_with = integer_default_zero_schema)]
    pub revision: i64,
    #[schema(schema_with = integer_default_zero_schema)]
    pub position_epoch: i64,
    #[schema(schema_with = boolean_default_false_schema)]
    pub is_playing: bool,
    #[schema(schema_with = number_default_one_schema)]
    pub volume: f64,
    #[schema(schema_with = nullable_string_schema)]
    pub active_mode_id: Option<String>,
    #[schema(schema_with = string_array_schema)]
    pub active_output_device_ids: Vec<String>,
    #[schema(schema_with = number_default_one_schema)]
    pub default_device_volume: f64,
    #[schema(schema_with = number_map_schema)]
    pub device_volumes: std::collections::BTreeMap<String, f64>,
    #[schema(schema_with = nullable_string_schema)]
    pub active_soundboard_id: Option<String>,
    #[schema(schema_with = string_array_schema)]
    pub active_preset_ids: Vec<String>,
    #[schema(schema_with = integer_default_zero_schema)]
    pub preset_revision: i64,
    #[schema(schema_with = integer_default_zero_schema)]
    pub crossfade_ms: i64,
    #[schema(schema_with = crossfade_type_schema)]
    pub crossfade_type: CrossfadeType,
    #[schema(schema_with = ambient_state_schema)]
    pub ambient: AmbientState,
    #[schema(schema_with = nullable_interrupt_state_schema)]
    pub interrupt: Option<InterruptState>,
    #[schema(schema_with = looping_sfx_array_schema)]
    pub looping_sfx: Vec<LoopingSfx>,
    #[schema(schema_with = nullable_position_report_schema)]
    pub last_position_report: Option<PositionReport>,
    #[schema(schema_with = device_info_array_schema)]
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

fn integer_schema() -> RefOr<Schema> {
    ObjectBuilder::new().schema_type(Type::Integer).into()
}

fn integer_default_zero_schema() -> RefOr<Schema> {
    ObjectBuilder::new()
        .schema_type(Type::Integer)
        .default(Some(serde_json::json!(0)))
        .into()
}

fn number_schema() -> RefOr<Schema> {
    ObjectBuilder::new().schema_type(Type::Number).into()
}

fn number_default_one_schema() -> RefOr<Schema> {
    ObjectBuilder::new()
        .schema_type(Type::Number)
        .default(Some(serde_json::json!(1.0)))
        .into()
}

fn boolean_default_false_schema() -> RefOr<Schema> {
    ObjectBuilder::new()
        .schema_type(Type::Boolean)
        .default(Some(serde_json::json!(false)))
        .into()
}

fn boolean_default_true_schema() -> RefOr<Schema> {
    ObjectBuilder::new()
        .schema_type(Type::Boolean)
        .default(Some(serde_json::json!(true)))
        .into()
}

fn nullable_integer_schema() -> RefOr<Schema> {
    Schema::AnyOf(
        AnyOfBuilder::new()
            .item(integer_schema())
            .item(ObjectBuilder::new().schema_type(Type::Null))
            .build(),
    )
    .into()
}

fn nullable_number_schema() -> RefOr<Schema> {
    Schema::AnyOf(
        AnyOfBuilder::new()
            .item(number_schema())
            .item(ObjectBuilder::new().schema_type(Type::Null))
            .build(),
    )
    .into()
}

fn nullable_unit_number_schema() -> RefOr<Schema> {
    Schema::AnyOf(
        AnyOfBuilder::new()
            .item(
                ObjectBuilder::new()
                    .schema_type(Type::Number)
                    .minimum(Some(0))
                    .maximum(Some(1)),
            )
            .item(ObjectBuilder::new().schema_type(Type::Null))
            .build(),
    )
    .into()
}

fn nullable_string_schema() -> RefOr<Schema> {
    Schema::AnyOf(
        AnyOfBuilder::new()
            .item(ObjectBuilder::new().schema_type(Type::String))
            .item(ObjectBuilder::new().schema_type(Type::Null))
            .build(),
    )
    .into()
}

fn integer_array_schema() -> RefOr<Schema> {
    ArrayBuilder::new().items(integer_schema()).into()
}

fn string_array_schema() -> RefOr<Schema> {
    ArrayBuilder::new()
        .items(ObjectBuilder::new().schema_type(Type::String))
        .into()
}

fn number_map_schema() -> RefOr<Schema> {
    ObjectBuilder::new()
        .schema_type(Type::Object)
        .additional_properties(Some(ObjectBuilder::new().schema_type(Type::Number)))
        .into()
}

fn loop_mode_schema() -> RefOr<Schema> {
    ObjectBuilder::new()
        .schema_type(Type::String)
        .enum_values(Some(["off", "follow", "queue", "track"]))
        .default(Some(serde_json::json!("off")))
        .into()
}

fn shuffle_mode_schema() -> RefOr<Schema> {
    ObjectBuilder::new()
        .schema_type(Type::String)
        .enum_values(Some(["off", "random"]))
        .default(Some(serde_json::json!("off")))
        .into()
}

fn crossfade_type_schema() -> RefOr<Schema> {
    ObjectBuilder::new()
        .schema_type(Type::String)
        .enum_values(Some(["linear", "equal_power", "cut"]))
        .default(Some(serde_json::json!("linear")))
        .into()
}

fn loop_interval_schema() -> RefOr<Schema> {
    ObjectBuilder::new()
        .schema_type(Type::Number)
        .minimum(Some(1))
        .maximum(Some(3_600))
        .into()
}

fn unit_number_default_one_schema() -> RefOr<Schema> {
    ObjectBuilder::new()
        .schema_type(Type::Number)
        .minimum(Some(0))
        .maximum(Some(1))
        .default(Some(serde_json::json!(1.0)))
        .into()
}

fn ambient_state_schema() -> RefOr<Schema> {
    RefOr::Ref(Ref::from_schema_name("AmbientState"))
}

fn nullable_interrupt_state_schema() -> RefOr<Schema> {
    Schema::AnyOf(
        AnyOfBuilder::new()
            .item(RefOr::Ref(Ref::from_schema_name("InterruptState")))
            .item(ObjectBuilder::new().schema_type(Type::Null))
            .build(),
    )
    .into()
}

fn nullable_position_report_schema() -> RefOr<Schema> {
    Schema::AnyOf(
        AnyOfBuilder::new()
            .item(RefOr::Ref(Ref::from_schema_name("PositionReport")))
            .item(ObjectBuilder::new().schema_type(Type::Null))
            .build(),
    )
    .into()
}

fn looping_sfx_array_schema() -> RefOr<Schema> {
    ArrayBuilder::new()
        .items(RefOr::Ref(Ref::from_schema_name("LoopingSfx")))
        .into()
}

fn device_info_array_schema() -> RefOr<Schema> {
    ArrayBuilder::new()
        .items(RefOr::Ref(Ref::from_schema_name("DeviceInfo")))
        .into()
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
