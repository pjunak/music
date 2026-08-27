use std::collections::BTreeMap;
use std::error::Error;
use std::fmt::{self, Display, Formatter};

use serde::de::DeserializeOwned;
use serde::{Deserialize, Serialize};
use serde_json::Value;

const MAX_YAML_INPUT_BYTES: usize = 1024 * 1024;
const MAX_SCALAR_BYTES: usize = 512 * 1024;
const MAX_COMMENT_BYTES: usize = 256 * 1024;

#[derive(Debug)]
pub enum YamlDocumentError {
    TooLarge {
        actual: usize,
        maximum: usize,
    },
    Parse(String),
    Serialize(String),
    IdMismatch {
        kind: &'static str,
        expected: String,
        actual: String,
    },
    InvalidValue {
        field: &'static str,
    },
}

impl Display for YamlDocumentError {
    fn fmt(&self, formatter: &mut Formatter<'_>) -> fmt::Result {
        match self {
            Self::TooLarge { actual, maximum } => {
                write!(
                    formatter,
                    "authored YAML is {actual} bytes; maximum is {maximum}"
                )
            }
            Self::Parse(message) => write!(formatter, "authored YAML is invalid: {message}"),
            Self::Serialize(message) => {
                write!(
                    formatter,
                    "authored YAML could not be serialized: {message}"
                )
            }
            Self::IdMismatch {
                kind,
                expected,
                actual,
            } => write!(
                formatter,
                "{kind} id '{actual}' does not match its location '{expected}'"
            ),
            Self::InvalidValue { field } => write!(formatter, "invalid authored value: {field}"),
        }
    }
}

impl Error for YamlDocumentError {}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct InterruptDocument {
    pub name: String,
    #[serde(default)]
    pub playlist: Option<String>,
    #[serde(default)]
    pub soundboard_item: Option<String>,
    #[serde(default)]
    pub fade_in_ms: i64,
    #[serde(default)]
    pub fade_out_ms: i64,
    #[serde(default = "default_true")]
    pub return_to_ambient: bool,
    #[serde(default)]
    pub duck_to: Option<f64>,
    #[serde(flatten)]
    pub extra: BTreeMap<String, Value>,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct IntegrationsDocument {
    #[serde(default)]
    pub lights: Option<BTreeMap<String, Value>>,
    #[serde(flatten)]
    pub extra: BTreeMap<String, Value>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ModeDocument {
    pub id: String,
    pub name: String,
    #[serde(default)]
    pub theme: Option<String>,
    #[serde(default)]
    pub panels: Vec<String>,
    #[serde(default)]
    pub playlist_categories: Vec<String>,
    #[serde(default)]
    pub interrupts: Vec<InterruptDocument>,
    #[serde(default)]
    pub integrations: IntegrationsDocument,
    #[serde(default)]
    pub default_crossfade_ms: i64,
    #[serde(default)]
    pub default_soundboard: Option<String>,
    #[serde(flatten)]
    pub extra: BTreeMap<String, Value>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SoundboardItemDocument {
    pub file: String,
    pub name: String,
    #[serde(default)]
    pub icon: Option<String>,
    #[serde(default)]
    pub hotkey: Option<String>,
    #[serde(flatten)]
    pub extra: BTreeMap<String, Value>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SoundboardCategoryDocument {
    pub id: String,
    pub name: String,
    #[serde(default)]
    pub items: Vec<SoundboardItemDocument>,
    #[serde(flatten)]
    pub extra: BTreeMap<String, Value>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SoundboardDocument {
    #[serde(default)]
    pub id: Option<String>,
    #[serde(default)]
    pub name: Option<String>,
    #[serde(default)]
    pub categories: Vec<SoundboardCategoryDocument>,
    #[serde(flatten)]
    pub extra: BTreeMap<String, Value>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct CueSfxDocument {
    pub soundboard: String,
    pub item: String,
    #[serde(default = "default_volume")]
    pub volume: f64,
    #[serde(flatten)]
    pub extra: BTreeMap<String, Value>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct CueLoopDocument {
    pub soundboard: String,
    pub item: String,
    pub interval_s: f64,
    #[serde(default = "default_volume")]
    pub volume: f64,
    #[serde(flatten)]
    pub extra: BTreeMap<String, Value>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct CueDocument {
    #[serde(default)]
    pub id: Option<String>,
    pub name: String,
    #[serde(default)]
    pub description: Option<String>,
    #[serde(default)]
    pub preset: Option<String>,
    #[serde(default)]
    pub playlist: Option<String>,
    #[serde(default)]
    pub start_index: u64,
    #[serde(default)]
    pub start_ms: u64,
    #[serde(default)]
    pub sfx: Vec<CueSfxDocument>,
    #[serde(default)]
    pub loops: Vec<CueLoopDocument>,
    #[serde(flatten)]
    pub extra: BTreeMap<String, Value>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct EffectDocument {
    #[serde(rename = "type")]
    pub effect_type: String,
    #[serde(flatten)]
    pub parameters: BTreeMap<String, Value>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct PresetDocument {
    #[serde(default)]
    pub id: Option<String>,
    pub name: String,
    #[serde(default)]
    pub description: Option<String>,
    #[serde(default)]
    pub effects: Vec<EffectDocument>,
    #[serde(default)]
    pub crossfade_ms: Option<u64>,
    #[serde(flatten)]
    pub extra: BTreeMap<String, Value>,
}

pub fn parse_mode_document(
    input: &str,
    expected_id: &str,
) -> Result<ModeDocument, YamlDocumentError> {
    let document: ModeDocument = deserialize_document(input)?;
    validate_required_id("mode", expected_id, &document.id)?;
    for interrupt in &document.interrupts {
        if let Some(duck_to) = interrupt.duck_to {
            validate_unit_interval("interrupts[].duck_to", duck_to)?;
        }
    }
    Ok(document)
}

pub fn parse_soundboard_document(
    input: &str,
    expected_id: &str,
) -> Result<SoundboardDocument, YamlDocumentError> {
    let mut document: SoundboardDocument = deserialize_document(input)?;
    resolve_optional_id("soundboard", expected_id, &mut document.id)?;
    Ok(document)
}

pub fn parse_cue_document(
    input: &str,
    expected_id: &str,
) -> Result<CueDocument, YamlDocumentError> {
    let mut document: CueDocument = deserialize_document(input)?;
    resolve_optional_id("cue", expected_id, &mut document.id)?;
    for sfx in &document.sfx {
        validate_unit_interval("sfx[].volume", sfx.volume)?;
    }
    for loop_spec in &document.loops {
        validate_range("loops[].interval_s", loop_spec.interval_s, 1.0, 3600.0)?;
        validate_unit_interval("loops[].volume", loop_spec.volume)?;
    }
    Ok(document)
}

pub fn parse_preset_document(
    input: &str,
    expected_id: &str,
) -> Result<PresetDocument, YamlDocumentError> {
    let mut document: PresetDocument = deserialize_document(input)?;
    resolve_optional_id("preset", expected_id, &mut document.id)?;
    if document.crossfade_ms.is_some_and(|value| value > 60_000) {
        return Err(YamlDocumentError::InvalidValue {
            field: "crossfade_ms",
        });
    }
    for effect in &document.effects {
        if !is_supported_effect(&effect.effect_type) {
            return Err(YamlDocumentError::InvalidValue {
                field: "effects[].type",
            });
        }
    }
    Ok(document)
}

pub fn serialize_document<T: Serialize>(document: &T) -> Result<String, YamlDocumentError> {
    let serialized = serde_saphyr::to_string(document)
        .map_err(|error| YamlDocumentError::Serialize(error.to_string()))?;
    validate_size(serialized.len())?;
    Ok(serialized)
}

fn deserialize_document<T: DeserializeOwned>(input: &str) -> Result<T, YamlDocumentError> {
    validate_size(input.len())?;
    serde_saphyr::from_str_with_options(input, parser_options())
        .map_err(|error| YamlDocumentError::Parse(error.to_string()))
}

fn parser_options() -> serde_saphyr::Options {
    serde_saphyr::options! {
        emit_comments: true,
        duplicate_keys: serde_saphyr::DuplicateKeyPolicy::Error,
        merge_keys: serde_saphyr::MergeKeyPolicy::Merge,
        strict_booleans: false,
        alias_limits: serde_saphyr::alias_limits! {
            max_total_replayed_events: 20_000,
            max_replay_stack_depth: 16,
            max_alias_expansions_per_anchor: 64,
        },
        budget: serde_saphyr::budget! {
            max_reader_input_bytes: Some(MAX_YAML_INPUT_BYTES),
            max_buffered_comment_events: 64,
            simple_key_max_lookahead: 1024,
            flow_nesting_limit: 32,
            max_events: 20_000,
            max_aliases: 64,
            max_anchors: 64,
            max_depth: 32,
            max_inclusion_depth: 0,
            max_documents: 1,
            max_nodes: 10_000,
            max_total_scalar_bytes: MAX_SCALAR_BYTES,
            max_total_comment_bytes: MAX_COMMENT_BYTES,
            max_merge_keys: 64,
            enforce_alias_anchor_ratio: true,
            alias_anchor_min_aliases: 16,
            alias_anchor_ratio_multiplier: 8,
        },
    }
}

fn validate_size(actual: usize) -> Result<(), YamlDocumentError> {
    if actual > MAX_YAML_INPUT_BYTES {
        return Err(YamlDocumentError::TooLarge {
            actual,
            maximum: MAX_YAML_INPUT_BYTES,
        });
    }
    Ok(())
}

fn validate_required_id(
    kind: &'static str,
    expected: &str,
    actual: &str,
) -> Result<(), YamlDocumentError> {
    if actual != expected {
        return Err(YamlDocumentError::IdMismatch {
            kind,
            expected: expected.to_owned(),
            actual: actual.to_owned(),
        });
    }
    Ok(())
}

fn resolve_optional_id(
    kind: &'static str,
    expected: &str,
    actual: &mut Option<String>,
) -> Result<(), YamlDocumentError> {
    match actual {
        Some(actual) => validate_required_id(kind, expected, actual),
        None => {
            *actual = Some(expected.to_owned());
            Ok(())
        }
    }
}

fn validate_unit_interval(field: &'static str, value: f64) -> Result<(), YamlDocumentError> {
    validate_range(field, value, 0.0, 1.0)
}

fn validate_range(
    field: &'static str,
    value: f64,
    minimum: f64,
    maximum: f64,
) -> Result<(), YamlDocumentError> {
    if !value.is_finite() || !(minimum..=maximum).contains(&value) {
        return Err(YamlDocumentError::InvalidValue { field });
    }
    Ok(())
}

fn is_supported_effect(effect_type: &str) -> bool {
    matches!(
        effect_type,
        "eq" | "reverb" | "lowpass" | "highpass" | "bandpass" | "delay" | "distortion" | "tremolo"
    )
}

const fn default_true() -> bool {
    true
}

const fn default_volume() -> f64 {
    1.0
}

#[cfg(test)]
mod tests {
    use std::error::Error;
    use std::io;
    use std::path::Path;

    use serde::Deserialize;
    use serde_json::Value;

    use super::{
        MAX_YAML_INPUT_BYTES, deserialize_document, parse_cue_document, parse_mode_document,
        parse_preset_document, parse_soundboard_document, serialize_document,
    };

    const AUTHORED_EXAMPLES: &str =
        include_str!("../../../contracts/reference/v1/authored-files.examples.json");

    #[derive(Deserialize)]
    struct AuthoredFixture {
        cases: Vec<AuthoredCase>,
    }

    #[derive(Deserialize)]
    struct AuthoredCase {
        canonical: Value,
        kind: String,
        path: String,
        source: String,
    }

    #[test]
    fn current_python_authored_corpus_round_trips_semantically() -> Result<(), Box<dyn Error>> {
        let fixture: AuthoredFixture = serde_json::from_str(AUTHORED_EXAMPLES)?;
        assert!(!fixture.cases.is_empty());
        for case in fixture.cases {
            let expected_id = expected_id(&case)?;
            let canonical = match case.kind.as_str() {
                "mode" => {
                    let document = parse_mode_document(&case.source, &expected_id)?;
                    let serialized = serialize_document(&document)?;
                    assert_eq!(parse_mode_document(&serialized, &expected_id)?, document);
                    serde_json::to_value(document)?
                }
                "soundboard" => {
                    let document = parse_soundboard_document(&case.source, &expected_id)?;
                    let serialized = serialize_document(&document)?;
                    assert_eq!(
                        parse_soundboard_document(&serialized, &expected_id)?,
                        document
                    );
                    serde_json::to_value(document)?
                }
                "cue" => {
                    let document = parse_cue_document(&case.source, &expected_id)?;
                    let serialized = serialize_document(&document)?;
                    assert_eq!(parse_cue_document(&serialized, &expected_id)?, document);
                    serde_json::to_value(document)?
                }
                "preset" => {
                    let document = parse_preset_document(&case.source, &expected_id)?;
                    let serialized = serialize_document(&document)?;
                    assert_eq!(parse_preset_document(&serialized, &expected_id)?, document);
                    serde_json::to_value(document)?
                }
                other => return Err(io::Error::other(format!("unknown kind: {other}")).into()),
            };
            assert_eq!(canonical, case.canonical, "{}", case.path);
        }
        Ok(())
    }

    #[test]
    fn rejects_oversized_duplicate_multidocument_and_deep_inputs() {
        let oversized = format!("id: fixture\nname: {}\n", "x".repeat(MAX_YAML_INPUT_BYTES));
        assert!(parse_mode_document(&oversized, "fixture").is_err());

        let duplicate = "id: fixture\nid: fixture\nname: Duplicate\n";
        assert!(parse_mode_document(duplicate, "fixture").is_err());

        let multiple = "id: fixture\nname: First\n---\nid: second\nname: Second\n";
        assert!(parse_mode_document(multiple, "fixture").is_err());

        let deep = format!("value: {}0{}", "[".repeat(33), "]".repeat(33));
        assert!(deserialize_document::<Value>(&deep).is_err());
    }

    #[test]
    fn rejects_alias_bombs_and_invalid_domain_ranges() {
        let aliases = (0..65).map(|_| "  - *base\n").collect::<String>();
        let alias_bomb = format!("base: &base value\nitems:\n{aliases}");
        assert!(deserialize_document::<Value>(&alias_bomb).is_err());

        let bad_cue = "name: Bad\nsfx:\n  - soundboard: board\n    item: hit\n    volume: 1.1\n";
        assert!(parse_cue_document(bad_cue, "bad").is_err());

        let bad_preset = "name: Bad\ncrossfade_ms: 60001\neffects:\n  - type: pitch_shift\n";
        assert!(parse_preset_document(bad_preset, "bad").is_err());
    }

    fn expected_id(case: &AuthoredCase) -> Result<String, io::Error> {
        let path = Path::new(&case.path);
        let component = if case.kind == "mode" {
            path.parent().and_then(Path::file_name)
        } else {
            path.file_stem()
        };
        component
            .and_then(|value| value.to_str())
            .map(str::to_owned)
            .ok_or_else(|| io::Error::other(format!("invalid fixture path: {}", case.path)))
    }
}
