use std::collections::BTreeMap;

use axum::Json;
use axum::body::Body;
use axum::extract::rejection::JsonRejection;
use axum::extract::{Path, State};
use axum::http::header::{CONTENT_DISPOSITION, CONTENT_TYPE};
use axum::http::{HeaderMap, HeaderValue};
use axum::response::{IntoResponse, Response};
use music_application::auth::SessionTouch;
use music_application::modes::{
    CueDocument, CueLoopDocument, CueSfxDocument, EffectDocument, IntegrationsDocument,
    InterruptDocument, ModeBundle, ModeCatalog, ModeCoordinatorHandle, PresetDocument,
    SoundboardCategoryDocument, SoundboardDocument, SoundboardItemDocument,
};
use music_application::playback::{CatalogGeneration, PlaybackActorError, ResolvedPlaybackCommand};
use music_domain::PlaybackCommand;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use utoipa::openapi::schema::{
    AdditionalProperties, AnyOfBuilder, ObjectBuilder, Schema, SchemaType, Type,
};
use utoipa::openapi::{Ref, RefOr};
use utoipa::{PartialSchema, ToSchema};
use utoipa_axum::router::OpenApiRouter;
use utoipa_axum::routes;

use crate::auth::{current_session, optional_session};
use crate::error::{ApiError, HttpValidationErrorBody, openapi_integer, openapi_nullable_string};
use crate::http::HttpState;

mod write;

#[derive(Debug, Clone, Serialize, ToSchema)]
struct ModeSummary {
    id: String,
    name: String,
    panels: Vec<String>,
    playlist_categories: Vec<String>,
    has_theme: bool,
    #[schema(schema_with = openapi_integer)]
    default_crossfade_ms: i64,
    #[schema(required = true, schema_with = openapi_nullable_string)]
    default_soundboard: Option<String>,
}

#[derive(Debug, Clone, Serialize, ToSchema)]
struct ModeDetail {
    id: String,
    name: String,
    panels: Vec<String>,
    playlist_categories: Vec<String>,
    has_theme: bool,
    #[schema(schema_with = openapi_integer)]
    default_crossfade_ms: i64,
    #[schema(required = true, schema_with = openapi_nullable_string)]
    default_soundboard: Option<String>,
    interrupts: Vec<InterruptSpec>,
    integrations: IntegrationsSpec,
    #[schema(schema_with = soundboard_map_schema)]
    soundboards: BTreeMap<String, SoundboardManifest>,
    #[schema(schema_with = cue_map_schema)]
    cues: BTreeMap<String, CueSpec>,
    #[schema(schema_with = preset_map_schema)]
    presets: BTreeMap<String, PresetManifest>,
}

#[derive(Debug, Clone, Serialize, ToSchema)]
struct InterruptSpec {
    name: String,
    #[schema(required = false, schema_with = openapi_nullable_string)]
    playlist: Option<String>,
    #[schema(required = false, schema_with = openapi_nullable_string)]
    soundboard_item: Option<String>,
    #[schema(required = false, schema_with = default_zero_integer_schema)]
    fade_in_ms: i64,
    #[schema(required = false, schema_with = default_zero_integer_schema)]
    fade_out_ms: i64,
    #[schema(required = false, default = true)]
    return_to_ambient: bool,
    #[schema(required = false, schema_with = nullable_unit_interval_schema)]
    duck_to: Option<f64>,
}

#[derive(Debug, Clone, Serialize, ToSchema)]
struct IntegrationsSpec {
    #[schema(required = false, schema_with = nullable_freeform_object_schema)]
    lights: Option<BTreeMap<String, Value>>,
}

#[derive(Debug, Clone, Serialize, ToSchema)]
pub(crate) struct SoundboardManifest {
    id: String,
    #[schema(required = false, schema_with = openapi_nullable_string)]
    name: Option<String>,
    #[schema(required = false)]
    categories: Vec<SoundboardCategory>,
}

#[derive(Debug, Clone, Serialize, ToSchema)]
struct SoundboardCategory {
    id: String,
    name: String,
    #[schema(required = false)]
    items: Vec<SoundboardItem>,
}

#[derive(Debug, Clone, Serialize, ToSchema)]
struct SoundboardItem {
    file: String,
    name: String,
    #[schema(required = false, schema_with = openapi_nullable_string)]
    icon: Option<String>,
    #[schema(required = false, schema_with = openapi_nullable_string)]
    hotkey: Option<String>,
}

#[derive(Debug, Clone, Serialize, ToSchema)]
struct CueSfx {
    soundboard: String,
    item: String,
    #[schema(required = false, schema_with = unit_interval_default_one_schema)]
    volume: f64,
}

#[derive(Debug, Clone, Serialize, ToSchema)]
struct CueLoop {
    soundboard: String,
    item: String,
    #[schema(schema_with = loop_interval_schema)]
    interval_s: f64,
    #[schema(required = false, schema_with = unit_interval_default_one_schema)]
    volume: f64,
}

#[derive(Debug, Clone, Serialize, ToSchema)]
pub(crate) struct CueSpec {
    id: String,
    name: String,
    #[schema(required = false, schema_with = openapi_nullable_string)]
    description: Option<String>,
    #[schema(required = false, schema_with = openapi_nullable_string)]
    preset: Option<String>,
    #[schema(required = false, schema_with = openapi_nullable_string)]
    playlist: Option<String>,
    #[schema(required = false, schema_with = nonnegative_default_zero_integer_schema)]
    start_index: u64,
    #[schema(required = false, schema_with = nonnegative_default_zero_integer_schema)]
    start_ms: u64,
    #[schema(required = false)]
    sfx: Vec<CueSfx>,
    #[schema(required = false)]
    loops: Vec<CueLoop>,
}

#[derive(Debug, Clone, Serialize)]
struct EffectSpec {
    #[serde(rename = "type")]
    effect_type: String,
    #[serde(flatten)]
    parameters: BTreeMap<String, Value>,
}

impl PartialSchema for EffectSpec {
    fn schema() -> RefOr<Schema> {
        ObjectBuilder::new()
            .schema_type(Type::Object)
            .property("type", ObjectBuilder::new().schema_type(Type::String))
            .required("type")
            .additional_properties(Some(AdditionalProperties::FreeForm(true)))
            .into()
    }
}

impl ToSchema for EffectSpec {}

#[derive(Debug, Clone, Serialize, ToSchema)]
struct PresetManifest {
    id: String,
    name: String,
    #[schema(required = false, schema_with = openapi_nullable_string)]
    description: Option<String>,
    #[schema(required = false)]
    effects: Vec<EffectSpec>,
    #[schema(required = false, schema_with = nullable_crossfade_schema)]
    crossfade_ms: Option<u64>,
}

#[derive(Debug, Clone, Serialize, ToSchema)]
struct ActiveMode {
    #[schema(required = true, schema_with = openapi_nullable_string)]
    mode_id: Option<String>,
}

#[derive(Debug, Deserialize)]
#[serde(transparent)]
struct RequiredNullableString(Option<String>);

#[derive(Debug, Deserialize, ToSchema)]
struct SetActiveModeRequest {
    #[schema(required = true, schema_with = openapi_nullable_string)]
    mode_id: RequiredNullableString,
}

#[derive(Debug, Clone, Serialize, ToSchema)]
struct ReloadResult {
    loaded: Vec<String>,
    #[schema(schema_with = string_map_schema)]
    errors: BTreeMap<String, String>,
}

struct ThemeFileResponse;

impl PartialSchema for ThemeFileResponse {
    fn schema() -> RefOr<Schema> {
        Schema::Object(
            ObjectBuilder::new()
                .schema_type(SchemaType::AnyValue)
                .build(),
        )
        .into()
    }
}

impl ToSchema for ThemeFileResponse {}

pub(crate) fn mode_router() -> OpenApiRouter<HttpState> {
    OpenApiRouter::default()
        .routes(routes!(list_modes))
        .routes(routes!(get_active_mode))
        .routes(routes!(set_active_mode))
        .routes(routes!(reload_modes))
        .routes(routes!(get_mode))
        .routes(routes!(list_mode_presets))
        .routes(routes!(get_mode_theme))
        .merge(write::mode_write_router())
}

#[utoipa::path(
    get,
    path = "/modes",
    responses((status = 200, description = "Successful Response", body = [ModeSummary])),
    tag = "modes"
)]
async fn list_modes(
    State(state): State<HttpState>,
    headers: HeaderMap,
) -> Result<Json<Vec<ModeSummary>>, ApiError> {
    current_session(&state, &headers, SessionTouch::UpdateLastSeen).await?;
    let catalog = mode_catalog(&state)?;
    Ok(Json(catalog.modes.values().map(summary).collect()))
}

#[utoipa::path(
    get,
    path = "/modes/active",
    responses((status = 200, description = "Successful Response", body = ActiveMode)),
    tag = "modes"
)]
async fn get_active_mode(
    State(state): State<HttpState>,
    headers: HeaderMap,
) -> Result<Json<ActiveMode>, ApiError> {
    current_session(&state, &headers, SessionTouch::UpdateLastSeen).await?;
    let playback = playback(&state)?;
    let snapshot = playback.snapshot().await.map_err(playback_unavailable)?;
    Ok(Json(ActiveMode {
        mode_id: snapshot.state.active_mode_id.clone(),
    }))
}

#[utoipa::path(
    put,
    path = "/modes/active",
    request_body = SetActiveModeRequest,
    responses(
        (status = 200, description = "Successful Response", body = ActiveMode),
        (status = 422, description = "Validation Error", body = HttpValidationErrorBody)
    ),
    tag = "modes"
)]
async fn set_active_mode(
    State(state): State<HttpState>,
    headers: HeaderMap,
    payload: Result<Json<SetActiveModeRequest>, JsonRejection>,
) -> Result<Json<ActiveMode>, ApiError> {
    current_session(&state, &headers, SessionTouch::UpdateLastSeen).await?;
    let Json(payload) = payload.map_err(|_| ApiError::validation())?;
    let mode_id = payload.mode_id.0;
    let playback = playback(&state)?;
    let mut result = None;
    for _ in 0..2 {
        let generation = catalog_generation(&state)?;
        match playback
            .execute(ResolvedPlaybackCommand::at_generation(
                PlaybackCommand::SetActiveMode(mode_id.clone()),
                generation,
            ))
            .await
        {
            Ok(_) => {
                result = Some(Ok(()));
                break;
            }
            Err(PlaybackActorError::StaleCatalog { .. }) => {
                tokio::task::yield_now().await;
            }
            Err(error) => {
                result = Some(Err(map_active_mode_error(error)));
                break;
            }
        }
    }
    match result {
        Some(Ok(())) => {}
        Some(Err(error)) => return Err(error),
        None => return Err(ApiError::conflict("catalog changed; retry the request")),
    }
    Ok(Json(ActiveMode { mode_id }))
}

#[utoipa::path(
    post,
    path = "/modes/reload",
    responses((status = 200, description = "Successful Response", body = ReloadResult)),
    tag = "modes"
)]
async fn reload_modes(
    State(state): State<HttpState>,
    headers: HeaderMap,
) -> Result<Json<ReloadResult>, ApiError> {
    current_session(&state, &headers, SessionTouch::UpdateLastSeen).await?;
    let result = modes(&state)?.reload().await.map_err(|error| {
        tracing::error!(error = %error, "mode reload failed");
        ApiError::service_unavailable()
    })?;
    Ok(Json(ReloadResult {
        loaded: result.loaded_ids,
        errors: result.errors,
    }))
}

#[utoipa::path(
    get,
    path = "/modes/{mode_id}",
    params(("mode_id" = String, Path)),
    responses(
        (status = 200, description = "Successful Response", body = ModeDetail),
        (status = 422, description = "Validation Error", body = HttpValidationErrorBody)
    ),
    tag = "modes"
)]
async fn get_mode(
    State(state): State<HttpState>,
    headers: HeaderMap,
    Path(mode_id): Path<String>,
) -> Result<Json<ModeDetail>, ApiError> {
    current_session(&state, &headers, SessionTouch::UpdateLastSeen).await?;
    let catalog = mode_catalog(&state)?;
    let mode = catalog
        .modes
        .get(&mode_id)
        .ok_or_else(|| ApiError::plain_not_found("mode not loaded"))?;
    Ok(Json(detail(mode)))
}

#[utoipa::path(
    get,
    path = "/modes/{mode_id}/presets",
    params(("mode_id" = String, Path)),
    responses(
        (status = 200, description = "Successful Response", body = [PresetManifest]),
        (status = 422, description = "Validation Error", body = HttpValidationErrorBody)
    ),
    tag = "modes"
)]
async fn list_mode_presets(
    State(state): State<HttpState>,
    headers: HeaderMap,
    Path(mode_id): Path<String>,
) -> Result<Json<Vec<PresetManifest>>, ApiError> {
    optional_session(&state, &headers, SessionTouch::UpdateLastSeen).await?;
    let catalog = mode_catalog(&state)?;
    let mode = catalog
        .modes
        .get(&mode_id)
        .ok_or_else(|| ApiError::plain_not_found("mode not loaded"))?;
    Ok(Json(
        mode.presets
            .iter()
            .map(|(id, preset)| preset_manifest(id, preset))
            .collect(),
    ))
}

#[utoipa::path(
    get,
    path = "/modes/{mode_id}/theme.css",
    params(("mode_id" = String, Path)),
    responses(
        (status = 200, description = "Successful Response", body = ThemeFileResponse),
        (status = 422, description = "Validation Error", body = HttpValidationErrorBody)
    ),
    tag = "modes"
)]
async fn get_mode_theme(
    State(state): State<HttpState>,
    headers: HeaderMap,
    Path(mode_id): Path<String>,
) -> Result<Response, ApiError> {
    current_session(&state, &headers, SessionTouch::UpdateLastSeen).await?;
    let catalog = mode_catalog(&state)?;
    let theme = catalog
        .modes
        .get(&mode_id)
        .ok_or_else(|| ApiError::plain_not_found("mode not loaded"))?
        .theme_css
        .as_ref()
        .ok_or_else(|| ApiError::plain_not_found("theme not declared or missing"))?;
    let mut response = Body::from(theme.to_string()).into_response();
    response.headers_mut().insert(
        CONTENT_TYPE,
        HeaderValue::from_static("text/css; charset=utf-8"),
    );
    response
        .headers_mut()
        .insert(CONTENT_DISPOSITION, HeaderValue::from_static("inline"));
    Ok(response)
}

fn modes(state: &HttpState) -> Result<&ModeCoordinatorHandle, ApiError> {
    state
        .modes
        .as_ref()
        .ok_or_else(ApiError::service_unavailable)
}

fn playback(
    state: &HttpState,
) -> Result<&music_application::playback::PlaybackActorHandle, ApiError> {
    state
        .playback
        .as_ref()
        .ok_or_else(ApiError::service_unavailable)
}

fn mode_catalog(state: &HttpState) -> Result<std::sync::Arc<ModeCatalog>, ApiError> {
    modes(state)?
        .snapshot()
        .ok_or_else(ApiError::service_unavailable)
}

fn catalog_generation(state: &HttpState) -> Result<CatalogGeneration, ApiError> {
    let library = state
        .library
        .as_ref()
        .ok_or_else(ApiError::service_unavailable)?;
    Ok(CatalogGeneration {
        library: library.coordinator.status().generation.get(),
        modes: mode_catalog(state)?.generation,
    })
}

fn map_active_mode_error(error: PlaybackActorError) -> ApiError {
    match error {
        PlaybackActorError::InvalidCatalogReference => ApiError::bad_request("unknown mode"),
        PlaybackActorError::StaleCatalog { .. } => {
            ApiError::conflict("catalog changed; retry the request")
        }
        other => playback_unavailable(other),
    }
}

fn playback_unavailable(error: PlaybackActorError) -> ApiError {
    tracing::error!(error = %error, "playback owner rejected a mode request");
    ApiError::service_unavailable()
}

fn summary(mode: &ModeBundle) -> ModeSummary {
    let manifest = &mode.manifest;
    ModeSummary {
        id: manifest.id.clone(),
        name: manifest.name.clone(),
        panels: manifest.panels.clone(),
        playlist_categories: manifest.playlist_categories.clone(),
        has_theme: mode.theme_css.is_some(),
        default_crossfade_ms: manifest.default_crossfade_ms,
        default_soundboard: manifest.default_soundboard.clone(),
    }
}

fn detail(mode: &ModeBundle) -> ModeDetail {
    let manifest = &mode.manifest;
    ModeDetail {
        id: manifest.id.clone(),
        name: manifest.name.clone(),
        panels: manifest.panels.clone(),
        playlist_categories: manifest.playlist_categories.clone(),
        has_theme: mode.theme_css.is_some(),
        default_crossfade_ms: manifest.default_crossfade_ms,
        default_soundboard: manifest.default_soundboard.clone(),
        interrupts: manifest.interrupts.iter().map(interrupt_spec).collect(),
        integrations: integrations_spec(&manifest.integrations),
        soundboards: mode
            .soundboards
            .iter()
            .map(|(id, soundboard)| (id.clone(), soundboard_manifest(id, soundboard)))
            .collect(),
        cues: mode
            .cues
            .iter()
            .map(|(id, cue)| (id.clone(), cue_spec(id, cue)))
            .collect(),
        presets: mode
            .presets
            .iter()
            .map(|(id, preset)| (id.clone(), preset_manifest(id, preset)))
            .collect(),
    }
}

fn interrupt_spec(document: &InterruptDocument) -> InterruptSpec {
    InterruptSpec {
        name: document.name.clone(),
        playlist: document.playlist.clone(),
        soundboard_item: document.soundboard_item.clone(),
        fade_in_ms: document.fade_in_ms,
        fade_out_ms: document.fade_out_ms,
        return_to_ambient: document.return_to_ambient,
        duck_to: document.duck_to,
    }
}

fn integrations_spec(document: &IntegrationsDocument) -> IntegrationsSpec {
    IntegrationsSpec {
        lights: document.lights.clone(),
    }
}

fn soundboard_manifest(id: &str, document: &SoundboardDocument) -> SoundboardManifest {
    SoundboardManifest {
        id: id.to_owned(),
        name: document.name.clone(),
        categories: document
            .categories
            .iter()
            .map(soundboard_category)
            .collect(),
    }
}

fn soundboard_category(document: &SoundboardCategoryDocument) -> SoundboardCategory {
    SoundboardCategory {
        id: document.id.clone(),
        name: document.name.clone(),
        items: document.items.iter().map(soundboard_item).collect(),
    }
}

fn soundboard_item(document: &SoundboardItemDocument) -> SoundboardItem {
    SoundboardItem {
        file: document.file.clone(),
        name: document.name.clone(),
        icon: document.icon.clone(),
        hotkey: document.hotkey.clone(),
    }
}

fn cue_spec(id: &str, document: &CueDocument) -> CueSpec {
    CueSpec {
        id: id.to_owned(),
        name: document.name.clone(),
        description: document.description.clone(),
        preset: document.preset.clone(),
        playlist: document.playlist.clone(),
        start_index: document.start_index,
        start_ms: document.start_ms,
        sfx: document.sfx.iter().map(cue_sfx).collect(),
        loops: document.loops.iter().map(cue_loop).collect(),
    }
}

fn cue_sfx(document: &CueSfxDocument) -> CueSfx {
    CueSfx {
        soundboard: document.soundboard.clone(),
        item: document.item.clone(),
        volume: document.volume,
    }
}

fn cue_loop(document: &CueLoopDocument) -> CueLoop {
    CueLoop {
        soundboard: document.soundboard.clone(),
        item: document.item.clone(),
        interval_s: document.interval_s,
        volume: document.volume,
    }
}

fn preset_manifest(id: &str, document: &PresetDocument) -> PresetManifest {
    PresetManifest {
        id: id.to_owned(),
        name: document.name.clone(),
        description: document.description.clone(),
        effects: document.effects.iter().map(effect_spec).collect(),
        crossfade_ms: document.crossfade_ms,
    }
}

fn effect_spec(document: &EffectDocument) -> EffectSpec {
    EffectSpec {
        effect_type: document.effect_type.clone(),
        parameters: document.parameters.clone(),
    }
}

fn nullable_freeform_object_schema() -> RefOr<Schema> {
    Schema::AnyOf(
        AnyOfBuilder::new()
            .item(
                ObjectBuilder::new()
                    .schema_type(Type::Object)
                    .additional_properties(Some(AdditionalProperties::FreeForm(true))),
            )
            .item(ObjectBuilder::new().schema_type(Type::Null))
            .build(),
    )
    .into()
}

fn string_map_schema() -> RefOr<Schema> {
    ObjectBuilder::new()
        .schema_type(Type::Object)
        .additional_properties(Some(ObjectBuilder::new().schema_type(Type::String)))
        .into()
}

fn soundboard_map_schema() -> RefOr<Schema> {
    typed_map_schema("SoundboardManifest")
}

fn cue_map_schema() -> RefOr<Schema> {
    typed_map_schema("CueSpec")
}

fn preset_map_schema() -> RefOr<Schema> {
    typed_map_schema("PresetManifest")
}

fn typed_map_schema(schema_name: &'static str) -> RefOr<Schema> {
    ObjectBuilder::new()
        .schema_type(Type::Object)
        .additional_properties(Some(Ref::from_schema_name(schema_name)))
        .into()
}

fn default_zero_integer_schema() -> RefOr<Schema> {
    ObjectBuilder::new()
        .schema_type(Type::Integer)
        .default(Some(serde_json::json!(0)))
        .into()
}

fn nonnegative_default_zero_integer_schema() -> RefOr<Schema> {
    ObjectBuilder::new()
        .schema_type(Type::Integer)
        .minimum(Some(0))
        .default(Some(serde_json::json!(0)))
        .into()
}

fn unit_interval_default_one_schema() -> RefOr<Schema> {
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

fn nullable_unit_interval_schema() -> RefOr<Schema> {
    nullable_schema(
        ObjectBuilder::new()
            .schema_type(Type::Number)
            .minimum(Some(0))
            .maximum(Some(1)),
    )
}

fn nullable_crossfade_schema() -> RefOr<Schema> {
    nullable_schema(
        ObjectBuilder::new()
            .schema_type(Type::Integer)
            .minimum(Some(0))
            .maximum(Some(60_000)),
    )
}

fn nullable_schema(value: ObjectBuilder) -> RefOr<Schema> {
    Schema::AnyOf(
        AnyOfBuilder::new()
            .item(value)
            .item(ObjectBuilder::new().schema_type(Type::Null))
            .build(),
    )
    .into()
}
