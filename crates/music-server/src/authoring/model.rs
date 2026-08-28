use std::collections::{BTreeMap, BTreeSet};

use serde::{Deserialize, Serialize};
use serde_json::Value;
use utoipa::openapi::extensions::Extensions;
use utoipa::openapi::schema::{
    AdditionalProperties, AnyOfBuilder, ArrayBuilder, ObjectBuilder, Schema, Type,
};
use utoipa::openapi::{Ref, RefOr};
use utoipa::{OpenApi, PartialSchema, ToSchema};

const SCHEMA_VERSION: &str = "authoring-import/v1";
const SLUG_PATTERN: &str = "^[a-z0-9][a-z0-9_-]*$";
const MAX_RESOURCES: usize = 500;
const MAX_TOTAL_TRACK_REFS: usize = 20_000;
const MAX_TOTAL_SOUNDS: usize = 20_000;
const MAX_TOTAL_CUE_ACTIONS: usize = 20_000;

#[derive(
    Debug, Clone, Copy, Deserialize, Eq, Hash, Ord, PartialEq, PartialOrd, Serialize, ToSchema,
)]
#[serde(rename_all = "lowercase")]
pub(super) enum AuthoringResourceKind {
    Playlist,
    Soundboard,
    Interrupt,
    Preset,
    Cue,
}

impl AuthoringResourceKind {
    pub(super) const fn as_str(self) -> &'static str {
        match self {
            Self::Playlist => "playlist",
            Self::Soundboard => "soundboard",
            Self::Interrupt => "interrupt",
            Self::Preset => "preset",
            Self::Cue => "cue",
        }
    }
}

#[derive(Debug, Clone, Copy, Eq, PartialEq, Serialize, ToSchema)]
#[serde(rename_all = "lowercase")]
pub(super) enum AuthoringSourceType {
    Mode,
    Document,
}

#[derive(Debug, Clone, Copy, Eq, PartialEq, Serialize, ToSchema)]
#[serde(rename_all = "lowercase")]
pub(super) enum ImportItemStatus {
    Ready,
    Conflict,
    Invalid,
}

#[derive(Debug, Clone, Copy, Eq, PartialEq, Serialize, ToSchema)]
#[serde(rename_all = "lowercase")]
pub(super) enum ImportIssueSeverity {
    Warning,
    Error,
}

#[derive(Debug, Clone, Eq, PartialEq, Serialize, ToSchema)]
#[serde(deny_unknown_fields)]
pub(super) struct AuthoringImportMode {
    pub(super) id: String,
    pub(super) name: String,
}

#[derive(Debug, Clone, Eq, PartialEq, Serialize, ToSchema)]
#[serde(deny_unknown_fields)]
pub(super) struct AuthoringImportSource {
    #[serde(rename = "type")]
    pub(super) source_type: AuthoringSourceType,
    pub(super) id: String,
    pub(super) name: String,
}

#[derive(Debug, Clone, Deserialize, Eq, Hash, Ord, PartialEq, PartialOrd, Serialize, ToSchema)]
#[serde(deny_unknown_fields)]
pub(super) struct AuthoringImportSelection {
    pub(super) kind: AuthoringResourceKind,
    #[schema(min_length = 1, max_length = 128)]
    pub(super) resource_id: String,
}

impl AuthoringImportSelection {
    pub(super) fn key(&self) -> String {
        format!("{}:{}", self.kind.as_str(), self.resource_id)
    }
}

#[derive(Debug, Clone, Eq, PartialEq, Serialize, ToSchema)]
#[serde(deny_unknown_fields)]
pub(super) struct AuthoringImportIssue {
    pub(super) code: String,
    pub(super) severity: ImportIssueSeverity,
    pub(super) message: String,
    #[schema(required = false, schema_with = nullable_selection_schema)]
    pub(super) related_item: Option<AuthoringImportSelection>,
}

#[derive(Debug, Clone, Eq, PartialEq, Serialize, ToSchema)]
#[serde(deny_unknown_fields)]
pub(super) struct AuthoringImportItem {
    pub(super) kind: AuthoringResourceKind,
    pub(super) resource_id: String,
    pub(super) name: String,
    pub(super) summary: String,
    pub(super) status: ImportItemStatus,
    #[schema(required = false, schema_with = nullable_string_schema)]
    pub(super) reason: Option<String>,
    #[serde(default)]
    pub(super) issues: Vec<AuthoringImportIssue>,
}

#[derive(Debug, Clone, Eq, PartialEq, Serialize, ToSchema)]
#[serde(deny_unknown_fields)]
pub(super) struct AuthoringImportPreview {
    pub(super) source: AuthoringImportSource,
    #[schema(required = false, schema_with = nullable_mode_schema)]
    pub(super) source_mode: Option<AuthoringImportMode>,
    pub(super) target_mode: AuthoringImportMode,
    pub(super) items: Vec<AuthoringImportItem>,
}

#[derive(Debug, Clone, Eq, PartialEq, Serialize, ToSchema)]
#[serde(deny_unknown_fields)]
pub(super) struct AuthoringImportResult {
    pub(super) imported: Vec<AuthoringImportItem>,
    pub(super) skipped: Vec<AuthoringImportItem>,
    pub(super) missing_track_paths: Vec<String>,
}

#[derive(Debug, Deserialize, ToSchema)]
#[serde(deny_unknown_fields)]
pub(super) struct AuthoringImportPreviewRequest {
    #[schema(min_length = 1, max_length = 64)]
    pub(super) source_mode_id: String,
    #[schema(min_length = 1, max_length = 64)]
    pub(super) target_mode_id: String,
}

#[derive(Debug, Deserialize, ToSchema)]
#[serde(deny_unknown_fields)]
pub(super) struct AuthoringImportCommitRequest {
    #[schema(min_length = 1, max_length = 64)]
    pub(super) source_mode_id: String,
    #[schema(min_length = 1, max_length = 64)]
    pub(super) target_mode_id: String,
    #[schema(min_items = 1, max_items = 500)]
    pub(super) items: Vec<AuthoringImportSelection>,
}

#[derive(Debug, Clone, Deserialize, ToSchema)]
#[serde(deny_unknown_fields)]
pub(super) struct ImportPlaylist {
    #[schema(min_length = 1, max_length = 256)]
    pub(super) name: String,
    #[schema(required = false, schema_with = nullable_category_schema)]
    pub(super) category: Option<String>,
    #[serde(default)]
    #[schema(schema_with = import_track_list_schema)]
    pub(super) tracks: Vec<String>,
}

#[derive(Debug, Clone, Deserialize, ToSchema)]
#[serde(deny_unknown_fields)]
pub(super) struct ImportSoundboardItem {
    #[schema(min_length = 1, max_length = 1024)]
    pub(super) file: String,
    #[schema(min_length = 1, max_length = 128)]
    pub(super) name: String,
    #[schema(required = false, schema_with = nullable_tiny_string_schema)]
    pub(super) icon: Option<String>,
    #[schema(required = false, schema_with = nullable_tiny_string_schema)]
    pub(super) hotkey: Option<String>,
}

#[derive(Debug, Clone, Deserialize, ToSchema)]
#[serde(deny_unknown_fields)]
pub(super) struct ImportSoundboardCategory {
    #[schema(min_length = 1, max_length = 64, pattern = "^[a-z0-9][a-z0-9_-]*$")]
    pub(super) id: String,
    #[schema(min_length = 1, max_length = 128)]
    pub(super) name: String,
    #[serde(default)]
    #[schema(max_items = 1000)]
    pub(super) items: Vec<ImportSoundboardItem>,
}

#[derive(Debug, Clone, Deserialize, ToSchema)]
#[serde(deny_unknown_fields)]
pub(super) struct ImportSoundboard {
    #[schema(min_length = 1, max_length = 64, pattern = "^[a-z0-9][a-z0-9_-]*$")]
    pub(super) id: String,
    #[schema(required = false, schema_with = nullable_short_name_schema)]
    pub(super) name: Option<String>,
    #[serde(default)]
    #[schema(max_items = 100)]
    pub(super) categories: Vec<ImportSoundboardCategory>,
}

#[derive(Debug, Clone, Deserialize, ToSchema)]
#[serde(deny_unknown_fields)]
pub(super) struct ImportInterrupt {
    #[schema(min_length = 1, max_length = 128)]
    pub(super) name: String,
    #[schema(required = false, schema_with = nullable_playlist_schema)]
    pub(super) playlist: Option<String>,
    #[schema(required = false, schema_with = nullable_path_schema)]
    pub(super) soundboard_item: Option<String>,
    #[serde(default)]
    #[schema(required = false, schema_with = default_fade_schema)]
    pub(super) fade_in_ms: i64,
    #[serde(default)]
    #[schema(required = false, schema_with = default_fade_schema)]
    pub(super) fade_out_ms: i64,
    #[serde(default = "default_true")]
    #[schema(required = false, default = true)]
    pub(super) return_to_ambient: bool,
    #[schema(required = false, schema_with = nullable_unit_interval_schema)]
    pub(super) duck_to: Option<f64>,
}

#[derive(Debug, Clone, Deserialize)]
pub(super) struct ImportEffect {
    #[serde(rename = "type")]
    pub(super) effect_type: String,
    #[serde(flatten)]
    pub(super) parameters: BTreeMap<String, Value>,
}

impl PartialSchema for ImportEffect {
    fn schema() -> RefOr<Schema> {
        ObjectBuilder::new()
            .schema_type(Type::Object)
            .property(
                "type",
                ObjectBuilder::new()
                    .schema_type(Type::String)
                    .min_length(Some(1))
                    .max_length(Some(64)),
            )
            .required("type")
            .additional_properties(Some(AdditionalProperties::FreeForm(true)))
            .into()
    }
}

impl ToSchema for ImportEffect {}

#[derive(Debug, Clone, Deserialize, ToSchema)]
#[serde(deny_unknown_fields)]
pub(super) struct ImportPreset {
    #[schema(min_length = 1, max_length = 64, pattern = "^[a-z0-9][a-z0-9_-]*$")]
    pub(super) id: String,
    #[schema(min_length = 1, max_length = 128)]
    pub(super) name: String,
    #[schema(required = false, schema_with = nullable_description_schema)]
    pub(super) description: Option<String>,
    #[serde(default)]
    #[schema(max_items = 32)]
    pub(super) effects: Vec<ImportEffect>,
    #[schema(required = false, schema_with = nullable_crossfade_schema)]
    pub(super) crossfade_ms: Option<u64>,
}

#[derive(Debug, Clone, Deserialize, ToSchema)]
#[serde(deny_unknown_fields)]
pub(super) struct ImportCueSfx {
    #[schema(min_length = 1, max_length = 64, pattern = "^[a-z0-9][a-z0-9_-]*$")]
    pub(super) soundboard: String,
    #[schema(min_length = 1, max_length = 1024)]
    pub(super) item: String,
    #[serde(default = "default_volume")]
    #[schema(required = false, schema_with = default_volume_schema)]
    pub(super) volume: f64,
}

#[derive(Debug, Clone, Deserialize, ToSchema)]
#[serde(deny_unknown_fields)]
pub(super) struct ImportCueLoop {
    #[schema(min_length = 1, max_length = 64, pattern = "^[a-z0-9][a-z0-9_-]*$")]
    pub(super) soundboard: String,
    #[schema(min_length = 1, max_length = 1024)]
    pub(super) item: String,
    #[schema(schema_with = loop_interval_schema)]
    pub(super) interval_s: f64,
    #[serde(default = "default_volume")]
    #[schema(required = false, schema_with = default_volume_schema)]
    pub(super) volume: f64,
}

#[derive(Debug, Clone, Deserialize, ToSchema)]
#[serde(deny_unknown_fields)]
pub(super) struct ImportCue {
    #[schema(min_length = 1, max_length = 64, pattern = "^[a-z0-9][a-z0-9_-]*$")]
    pub(super) id: String,
    #[schema(min_length = 1, max_length = 128)]
    pub(super) name: String,
    #[schema(required = false, schema_with = nullable_description_schema)]
    pub(super) description: Option<String>,
    #[schema(required = false, schema_with = nullable_slug_schema)]
    pub(super) preset: Option<String>,
    #[schema(required = false, schema_with = nullable_playlist_schema)]
    pub(super) playlist: Option<String>,
    #[serde(default)]
    #[schema(required = false, schema_with = default_start_index_schema)]
    pub(super) start_index: u64,
    #[serde(default)]
    #[schema(required = false, schema_with = default_start_ms_schema)]
    pub(super) start_ms: u64,
    #[serde(default)]
    #[schema(max_items = 500)]
    pub(super) sfx: Vec<ImportCueSfx>,
    #[serde(default)]
    #[schema(max_items = 500)]
    pub(super) loops: Vec<ImportCueLoop>,
}

#[derive(Debug, Clone, Deserialize, ToSchema)]
#[serde(deny_unknown_fields)]
pub(super) struct AuthoringImportDocumentV1 {
    #[serde(rename = "schema")]
    #[schema(schema_with = schema_version_schema)]
    pub(super) schema_version: String,
    #[schema(required = false, schema_with = nullable_short_name_schema)]
    pub(super) name: Option<String>,
    #[serde(default)]
    #[schema(max_items = 500)]
    pub(super) playlists: Vec<ImportPlaylist>,
    #[serde(default)]
    #[schema(max_items = 500)]
    pub(super) soundboards: Vec<ImportSoundboard>,
    #[serde(default)]
    #[schema(max_items = 500)]
    pub(super) interrupts: Vec<ImportInterrupt>,
    #[serde(default)]
    #[schema(max_items = 500)]
    pub(super) presets: Vec<ImportPreset>,
    #[serde(default)]
    #[schema(max_items = 500)]
    pub(super) cues: Vec<ImportCue>,
}

#[derive(Debug, Deserialize, ToSchema)]
#[serde(deny_unknown_fields)]
pub(super) struct AuthoringDocumentPreviewRequest {
    #[schema(min_length = 1, max_length = 64)]
    pub(super) target_mode_id: String,
    #[schema(required = false, schema_with = nullable_source_name_schema)]
    pub(super) source_name: Option<String>,
    pub(super) document: AuthoringImportDocumentV1,
}

#[derive(Debug, Deserialize, ToSchema)]
#[serde(deny_unknown_fields)]
pub(super) struct AuthoringDocumentCommitRequest {
    #[schema(min_length = 1, max_length = 64)]
    pub(super) target_mode_id: String,
    #[schema(required = false, schema_with = nullable_source_name_schema)]
    pub(super) source_name: Option<String>,
    pub(super) document: AuthoringImportDocumentV1,
    #[schema(min_items = 1, max_items = 500)]
    pub(super) items: Vec<AuthoringImportSelection>,
}

#[derive(Debug, Clone, Copy, Eq, PartialEq)]
pub(super) struct DocumentValidationError;

impl AuthoringImportPreviewRequest {
    pub(super) fn validate(&self) -> Result<(), DocumentValidationError> {
        validate_text(&self.source_mode_id, 1, 64)?;
        validate_text(&self.target_mode_id, 1, 64)
    }
}

impl AuthoringImportCommitRequest {
    pub(super) fn validate(&self) -> Result<(), DocumentValidationError> {
        validate_text(&self.source_mode_id, 1, 64)?;
        validate_text(&self.target_mode_id, 1, 64)?;
        validate_selections(&self.items)
    }
}

impl AuthoringDocumentPreviewRequest {
    pub(super) fn validate(&self) -> Result<(), DocumentValidationError> {
        validate_text(&self.target_mode_id, 1, 64)?;
        validate_optional_text(self.source_name.as_deref(), 0, 255)?;
        self.document.validate()
    }
}

impl AuthoringDocumentCommitRequest {
    pub(super) fn validate(&self) -> Result<(), DocumentValidationError> {
        validate_text(&self.target_mode_id, 1, 64)?;
        validate_optional_text(self.source_name.as_deref(), 0, 255)?;
        self.document.validate()?;
        validate_selections(&self.items)
    }
}

impl AuthoringImportDocumentV1 {
    fn validate(&self) -> Result<(), DocumentValidationError> {
        if self.schema_version != SCHEMA_VERSION {
            return Err(DocumentValidationError);
        }
        validate_optional_text(self.name.as_deref(), 0, 128)?;
        let resource_count = self.playlists.len()
            + self.soundboards.len()
            + self.interrupts.len()
            + self.presets.len()
            + self.cues.len();
        if !(1..=MAX_RESOURCES).contains(&resource_count)
            || self
                .playlists
                .iter()
                .map(|item| item.tracks.len())
                .sum::<usize>()
                > MAX_TOTAL_TRACK_REFS
            || self
                .soundboards
                .iter()
                .flat_map(|board| &board.categories)
                .map(|category| category.items.len())
                .sum::<usize>()
                > MAX_TOTAL_SOUNDS
            || self
                .cues
                .iter()
                .map(|cue| cue.sfx.len() + cue.loops.len())
                .sum::<usize>()
                > MAX_TOTAL_CUE_ACTIONS
        {
            return Err(DocumentValidationError);
        }

        unique(self.playlists.iter().map(|item| item.name.as_str()))?;
        unique(self.interrupts.iter().map(|item| item.name.as_str()))?;
        unique(self.soundboards.iter().map(|item| item.id.as_str()))?;
        unique(self.presets.iter().map(|item| item.id.as_str()))?;
        unique(self.cues.iter().map(|item| item.id.as_str()))?;

        for playlist in &self.playlists {
            validate_text(&playlist.name, 1, 256)?;
            validate_optional_text(playlist.category.as_deref(), 0, 64)?;
            if playlist.tracks.len() > 10_000 {
                return Err(DocumentValidationError);
            }
            for path in &playlist.tracks {
                validate_text(path, 1, 1024)?;
            }
        }
        for soundboard in &self.soundboards {
            validate_slug(&soundboard.id)?;
            validate_optional_text(soundboard.name.as_deref(), 0, 128)?;
            if soundboard.categories.len() > 100 {
                return Err(DocumentValidationError);
            }
            unique(soundboard.categories.iter().map(|item| item.id.as_str()))?;
            for category in &soundboard.categories {
                validate_slug(&category.id)?;
                validate_text(&category.name, 1, 128)?;
                if category.items.len() > 1000 {
                    return Err(DocumentValidationError);
                }
                for item in &category.items {
                    validate_text(&item.file, 1, 1024)?;
                    validate_text(&item.name, 1, 128)?;
                    validate_optional_text(item.icon.as_deref(), 0, 16)?;
                    validate_optional_text(item.hotkey.as_deref(), 0, 16)?;
                }
            }
        }
        for interrupt in &self.interrupts {
            validate_text(&interrupt.name, 1, 128)?;
            validate_optional_text(interrupt.playlist.as_deref(), 0, 256)?;
            validate_optional_text(interrupt.soundboard_item.as_deref(), 0, 1024)?;
            if interrupt
                .playlist
                .as_ref()
                .is_some_and(|value| !value.is_empty())
                == interrupt
                    .soundboard_item
                    .as_ref()
                    .is_some_and(|value| !value.is_empty())
                || !(0..=60_000).contains(&interrupt.fade_in_ms)
                || !(0..=60_000).contains(&interrupt.fade_out_ms)
                || interrupt
                    .duck_to
                    .is_some_and(|value| !value.is_finite() || !(0.0..=1.0).contains(&value))
            {
                return Err(DocumentValidationError);
            }
        }
        for preset in &self.presets {
            validate_slug(&preset.id)?;
            validate_text(&preset.name, 1, 128)?;
            validate_optional_text(preset.description.as_deref(), 0, 2000)?;
            if preset.effects.len() > 32 || preset.crossfade_ms.is_some_and(|value| value > 60_000)
            {
                return Err(DocumentValidationError);
            }
            for effect in &preset.effects {
                validate_text(&effect.effect_type, 1, 64)?;
            }
        }
        for cue in &self.cues {
            validate_slug(&cue.id)?;
            validate_text(&cue.name, 1, 128)?;
            validate_optional_text(cue.description.as_deref(), 0, 2000)?;
            if let Some(preset) = &cue.preset {
                validate_slug(preset)?;
            }
            validate_optional_text(cue.playlist.as_deref(), 0, 256)?;
            if cue.start_index > 100_000 || cue.sfx.len() > 500 || cue.loops.len() > 500 {
                return Err(DocumentValidationError);
            }
            for sfx in &cue.sfx {
                validate_cue_sound(&sfx.soundboard, &sfx.item, sfx.volume)?;
            }
            for loop_spec in &cue.loops {
                validate_cue_sound(&loop_spec.soundboard, &loop_spec.item, loop_spec.volume)?;
                if !loop_spec.interval_s.is_finite()
                    || !(1.0..=3600.0).contains(&loop_spec.interval_s)
                {
                    return Err(DocumentValidationError);
                }
            }
        }
        Ok(())
    }
}

fn validate_cue_sound(
    soundboard: &str,
    item: &str,
    volume: f64,
) -> Result<(), DocumentValidationError> {
    validate_slug(soundboard)?;
    validate_text(item, 1, 1024)?;
    if !volume.is_finite() || !(0.0..=1.0).contains(&volume) {
        return Err(DocumentValidationError);
    }
    Ok(())
}

fn validate_selections(
    selections: &[AuthoringImportSelection],
) -> Result<(), DocumentValidationError> {
    if !(1..=500).contains(&selections.len()) {
        return Err(DocumentValidationError);
    }
    for selection in selections {
        validate_text(&selection.resource_id, 1, 128)?;
    }
    Ok(())
}

fn validate_text(
    value: &str,
    minimum: usize,
    maximum: usize,
) -> Result<(), DocumentValidationError> {
    let length = value.chars().count();
    if length < minimum || length > maximum {
        return Err(DocumentValidationError);
    }
    Ok(())
}

fn validate_optional_text(
    value: Option<&str>,
    minimum: usize,
    maximum: usize,
) -> Result<(), DocumentValidationError> {
    value.map_or(Ok(()), |value| validate_text(value, minimum, maximum))
}

fn validate_slug(value: &str) -> Result<(), DocumentValidationError> {
    validate_text(value, 1, 64)?;
    let mut characters = value.chars();
    let first = characters.next().ok_or(DocumentValidationError)?;
    if !(first.is_ascii_lowercase() || first.is_ascii_digit())
        || !characters.all(|character| {
            character.is_ascii_lowercase()
                || character.is_ascii_digit()
                || matches!(character, '-' | '_')
        })
    {
        return Err(DocumentValidationError);
    }
    Ok(())
}

fn unique<'a>(values: impl IntoIterator<Item = &'a str>) -> Result<(), DocumentValidationError> {
    let mut seen = BTreeSet::new();
    if values.into_iter().all(|value| seen.insert(value)) {
        Ok(())
    } else {
        Err(DocumentValidationError)
    }
}

#[derive(OpenApi)]
#[openapi(components(schemas(AuthoringImportDocumentV1)))]
struct DocumentSchemaApi;

pub(super) fn public_document_schema() -> Result<Value, serde_json::Error> {
    let document = serde_json::to_value(DocumentSchemaApi::openapi())?;
    let mut schemas = document
        .pointer("/components/schemas")
        .and_then(Value::as_object)
        .cloned()
        .unwrap_or_default();
    let mut root = schemas
        .remove("AuthoringImportDocumentV1")
        .unwrap_or_else(|| Value::Object(Default::default()));
    rewrite_schema_references(&mut root);
    for schema in schemas.values_mut() {
        rewrite_schema_references(schema);
    }
    if let Some(root) = root.as_object_mut() {
        root.insert("$defs".to_owned(), Value::Object(schemas));
    }
    Ok(root)
}

fn rewrite_schema_references(value: &mut Value) {
    match value {
        Value::Object(object) => {
            if let Some(Value::String(reference)) = object.get_mut("$ref")
                && let Some(name) = reference.strip_prefix("#/components/schemas/")
            {
                *reference = format!("#/$defs/{name}");
            }
            for value in object.values_mut() {
                rewrite_schema_references(value);
            }
        }
        Value::Array(values) => {
            for value in values {
                rewrite_schema_references(value);
            }
        }
        _ => {}
    }
}

fn schema_version_schema() -> RefOr<Schema> {
    let extensions: Extensions = [("const", Value::String(SCHEMA_VERSION.to_owned()))]
        .into_iter()
        .collect();
    ObjectBuilder::new()
        .schema_type(Type::String)
        .extensions(Some(extensions))
        .into()
}

fn nullable_selection_schema() -> RefOr<Schema> {
    nullable_reference("AuthoringImportSelection")
}

fn import_track_list_schema() -> RefOr<Schema> {
    ArrayBuilder::new()
        .items(
            ObjectBuilder::new()
                .schema_type(Type::String)
                .min_length(Some(1))
                .max_length(Some(1024)),
        )
        .max_items(Some(10_000))
        .into()
}

fn nullable_mode_schema() -> RefOr<Schema> {
    nullable_reference("AuthoringImportMode")
}

fn nullable_reference(name: &'static str) -> RefOr<Schema> {
    Schema::AnyOf(
        AnyOfBuilder::new()
            .item(Ref::from_schema_name(name))
            .item(ObjectBuilder::new().schema_type(Type::Null))
            .build(),
    )
    .into()
}

fn nullable_string_schema() -> RefOr<Schema> {
    nullable_text_schema(None, None, None)
}

fn nullable_category_schema() -> RefOr<Schema> {
    nullable_text_schema(None, Some(64), None)
}

fn nullable_tiny_string_schema() -> RefOr<Schema> {
    nullable_text_schema(None, Some(16), None)
}

fn nullable_short_name_schema() -> RefOr<Schema> {
    nullable_text_schema(None, Some(128), None)
}

fn nullable_source_name_schema() -> RefOr<Schema> {
    nullable_text_schema(None, Some(255), None)
}

fn nullable_description_schema() -> RefOr<Schema> {
    nullable_text_schema(None, Some(2000), None)
}

fn nullable_playlist_schema() -> RefOr<Schema> {
    nullable_text_schema(None, Some(256), None)
}

fn nullable_path_schema() -> RefOr<Schema> {
    nullable_text_schema(None, Some(1024), None)
}

fn nullable_slug_schema() -> RefOr<Schema> {
    nullable_text_schema(Some(1), Some(64), Some(SLUG_PATTERN))
}

fn nullable_text_schema(
    minimum: Option<usize>,
    maximum: Option<usize>,
    pattern: Option<&str>,
) -> RefOr<Schema> {
    let mut text = ObjectBuilder::new().schema_type(Type::String);
    if let Some(minimum) = minimum {
        text = text.min_length(Some(minimum));
    }
    if let Some(maximum) = maximum {
        text = text.max_length(Some(maximum));
    }
    if let Some(pattern) = pattern {
        text = text.pattern(Some(pattern));
    }
    Schema::AnyOf(
        AnyOfBuilder::new()
            .item(text)
            .item(ObjectBuilder::new().schema_type(Type::Null))
            .build(),
    )
    .into()
}

fn nullable_unit_interval_schema() -> RefOr<Schema> {
    nullable_number_schema(Type::Number, 0, 1)
}

fn nullable_crossfade_schema() -> RefOr<Schema> {
    nullable_number_schema(Type::Integer, 0, 60_000)
}

fn nullable_number_schema(kind: Type, minimum: i64, maximum: i64) -> RefOr<Schema> {
    Schema::AnyOf(
        AnyOfBuilder::new()
            .item(
                ObjectBuilder::new()
                    .schema_type(kind)
                    .minimum(Some(minimum))
                    .maximum(Some(maximum)),
            )
            .item(ObjectBuilder::new().schema_type(Type::Null))
            .build(),
    )
    .into()
}

fn default_fade_schema() -> RefOr<Schema> {
    ObjectBuilder::new()
        .schema_type(Type::Integer)
        .minimum(Some(0))
        .maximum(Some(60_000))
        .default(Some(Value::from(0)))
        .into()
}

fn default_volume_schema() -> RefOr<Schema> {
    ObjectBuilder::new()
        .schema_type(Type::Number)
        .minimum(Some(0))
        .maximum(Some(1))
        .default(Some(serde_json::json!(1.0)))
        .into()
}

fn loop_interval_schema() -> RefOr<Schema> {
    ObjectBuilder::new()
        .schema_type(Type::Number)
        .minimum(Some(1))
        .maximum(Some(3600))
        .into()
}

fn default_start_index_schema() -> RefOr<Schema> {
    ObjectBuilder::new()
        .schema_type(Type::Integer)
        .minimum(Some(0))
        .maximum(Some(100_000))
        .default(Some(Value::from(0)))
        .into()
}

fn default_start_ms_schema() -> RefOr<Schema> {
    ObjectBuilder::new()
        .schema_type(Type::Integer)
        .minimum(Some(0))
        .default(Some(Value::from(0)))
        .into()
}

const fn default_true() -> bool {
    true
}

const fn default_volume() -> f64 {
    1.0
}
