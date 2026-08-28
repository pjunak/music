use std::collections::BTreeMap;
use std::sync::Arc;

use axum::Json;
use axum::extract::rejection::JsonRejection;
use axum::extract::{Path, State};
use axum::http::{HeaderMap, StatusCode};
use music_application::auth::SessionTouch;
use music_application::modes::{
    CueDocument, CueLoopDocument, CueSfxDocument, EffectDocument, IntegrationsDocument,
    InterruptDocument, ModeCatalog, ModeDocument, ModeMutation, ModeMutationError,
    ModeMutationFailureKind, PresetDocument, SoundboardCategoryDocument, SoundboardDocument,
    SoundboardItemDocument,
};
use serde::{Deserialize, Deserializer};
use serde_json::Value;
use utoipa::openapi::RefOr;
use utoipa::openapi::schema::{AdditionalProperties, AnyOfBuilder, ObjectBuilder, Schema, Type};
use utoipa::{PartialSchema, ToSchema};
use utoipa_axum::router::OpenApiRouter;
use utoipa_axum::routes;

use super::{
    CueSpec, InterruptSpec, ModeSummary, PresetManifest, SoundboardManifest, cue_spec,
    interrupt_spec, mode_catalog, preset_manifest, soundboard_manifest, summary,
};
use crate::auth::current_session;
use crate::error::{ApiError, HttpValidationErrorBody};
use crate::http::HttpState;

#[derive(Debug, Deserialize, ToSchema)]
struct CreateModeRequest {
    #[schema(min_length = 1, max_length = 64)]
    id: String,
    #[schema(min_length = 1, max_length = 128)]
    name: String,
}

#[derive(Debug, Deserialize, ToSchema)]
struct RenameModeRequest {
    #[schema(min_length = 1, max_length = 128)]
    name: String,
}

#[derive(Debug, Deserialize, ToSchema)]
struct CreateSoundboardRequest {
    #[schema(min_length = 1, max_length = 64)]
    id: String,
    #[schema(required = false, schema_with = nullable_short_name_schema)]
    name: Option<String>,
}

#[derive(Debug, Deserialize, ToSchema)]
struct AddCategoryRequest {
    #[schema(min_length = 1, max_length = 64)]
    id: String,
    #[schema(min_length = 1, max_length = 128)]
    name: String,
}

#[derive(Debug, Deserialize, ToSchema)]
struct AddItemRequest {
    #[schema(min_length = 1)]
    file: String,
    #[schema(min_length = 1, max_length = 128)]
    name: String,
    #[schema(required = false, schema_with = nullable_tiny_string_schema)]
    hotkey: Option<String>,
    #[schema(required = false, schema_with = nullable_tiny_string_schema)]
    icon: Option<String>,
}

#[derive(Debug, Deserialize, ToSchema)]
struct UpdateItemRequest {
    #[serde(default)]
    #[schema(required = false, schema_with = nullable_short_name_schema)]
    name: Patch<String>,
    #[serde(default)]
    #[schema(required = false, schema_with = nullable_tiny_string_schema)]
    hotkey: Patch<String>,
    #[serde(default)]
    #[schema(required = false, schema_with = nullable_tiny_string_schema)]
    icon: Patch<String>,
    #[serde(default)]
    #[schema(required = false, schema_with = nullable_string_schema)]
    file: Patch<String>,
}

#[derive(Debug, Deserialize, ToSchema)]
struct InterruptTemplateCreate {
    #[schema(min_length = 1, max_length = 128)]
    name: String,
    #[schema(required = false, schema_with = nullable_playlist_schema)]
    playlist: Option<String>,
    #[schema(required = false, schema_with = nullable_item_reference_schema)]
    soundboard_item: Option<String>,
    #[serde(default)]
    #[schema(required = false, schema_with = default_fade_schema)]
    fade_in_ms: i64,
    #[serde(default)]
    #[schema(required = false, schema_with = default_fade_schema)]
    fade_out_ms: i64,
    #[serde(default = "default_true")]
    #[schema(required = false, default = true)]
    return_to_ambient: bool,
    #[schema(required = false, schema_with = nullable_unit_interval_schema)]
    duck_to: Option<f64>,
}

#[derive(Debug, Deserialize, ToSchema)]
struct InterruptTemplateUpdate {
    #[serde(default)]
    #[schema(required = false, schema_with = nullable_nonempty_short_name_schema)]
    name: Patch<String>,
    #[serde(default)]
    #[schema(required = false, schema_with = nullable_playlist_schema)]
    playlist: Patch<String>,
    #[serde(default)]
    #[schema(required = false, schema_with = nullable_item_reference_schema)]
    soundboard_item: Patch<String>,
    #[serde(default)]
    #[schema(required = false, schema_with = nullable_fade_schema)]
    fade_in_ms: Patch<i64>,
    #[serde(default)]
    #[schema(required = false, schema_with = nullable_fade_schema)]
    fade_out_ms: Patch<i64>,
    #[serde(default)]
    #[schema(required = false, schema_with = nullable_boolean_schema)]
    return_to_ambient: Patch<bool>,
    #[serde(default)]
    #[schema(required = false, schema_with = nullable_unit_interval_schema)]
    duck_to: Patch<f64>,
}

#[derive(Debug, Clone, Deserialize, ToSchema)]
struct CueSfxIn {
    #[schema(min_length = 1, max_length = 128)]
    soundboard: String,
    #[schema(min_length = 1, max_length = 512)]
    item: String,
    #[serde(default = "default_volume")]
    #[schema(required = false, schema_with = super::unit_interval_default_one_schema)]
    volume: f64,
}

#[derive(Debug, Clone, Deserialize, ToSchema)]
struct CueLoopIn {
    #[schema(min_length = 1, max_length = 128)]
    soundboard: String,
    #[schema(min_length = 1, max_length = 512)]
    item: String,
    #[schema(schema_with = super::loop_interval_schema)]
    interval_s: f64,
    #[serde(default = "default_volume")]
    #[schema(required = false, schema_with = super::unit_interval_default_one_schema)]
    volume: f64,
}

#[derive(Debug, Deserialize, ToSchema)]
#[schema(as = CueBody)]
struct CueBodyRequest {
    #[schema(min_length = 1, max_length = 128)]
    name: String,
    #[schema(required = false, schema_with = nullable_string_schema)]
    description: Option<String>,
    #[schema(required = false, schema_with = nullable_slug_reference_schema)]
    preset: Option<String>,
    #[schema(required = false, schema_with = nullable_playlist_schema)]
    playlist: Option<String>,
    #[serde(default)]
    #[schema(required = false, schema_with = super::nonnegative_default_zero_integer_schema)]
    start_index: u64,
    #[serde(default)]
    #[schema(required = false, schema_with = super::nonnegative_default_zero_integer_schema)]
    start_ms: u64,
    #[serde(default)]
    #[schema(required = false)]
    sfx: Vec<CueSfxIn>,
    #[serde(default)]
    #[schema(required = false)]
    loops: Vec<CueLoopIn>,
}

#[derive(Debug, Deserialize, ToSchema)]
struct CreateCueRequest {
    #[schema(min_length = 1, max_length = 64)]
    id: String,
    #[schema(min_length = 1, max_length = 128)]
    name: String,
    #[schema(required = false, schema_with = nullable_string_schema)]
    description: Option<String>,
    #[schema(required = false, schema_with = nullable_slug_reference_schema)]
    preset: Option<String>,
    #[schema(required = false, schema_with = nullable_playlist_schema)]
    playlist: Option<String>,
    #[serde(default)]
    #[schema(required = false, schema_with = super::nonnegative_default_zero_integer_schema)]
    start_index: u64,
    #[serde(default)]
    #[schema(required = false, schema_with = super::nonnegative_default_zero_integer_schema)]
    start_ms: u64,
    #[serde(default)]
    #[schema(required = false)]
    sfx: Vec<CueSfxIn>,
    #[serde(default)]
    #[schema(required = false)]
    loops: Vec<CueLoopIn>,
}

#[derive(Debug, Clone, Deserialize)]
struct EffectSpecIn {
    #[serde(rename = "type")]
    effect_type: String,
    #[serde(flatten)]
    parameters: BTreeMap<String, Value>,
}

impl PartialSchema for EffectSpecIn {
    fn schema() -> RefOr<Schema> {
        ObjectBuilder::new()
            .schema_type(Type::Object)
            .property(
                "type",
                ObjectBuilder::new()
                    .schema_type(Type::String)
                    .min_length(Some(1)),
            )
            .required("type")
            .additional_properties(Some(AdditionalProperties::FreeForm(true)))
            .into()
    }
}

impl ToSchema for EffectSpecIn {}

#[derive(Debug, Deserialize, ToSchema)]
struct PresetBodyRequest {
    #[schema(min_length = 1, max_length = 128)]
    name: String,
    #[schema(required = false, schema_with = nullable_string_schema)]
    description: Option<String>,
    #[serde(default)]
    #[schema(required = false)]
    effects: Vec<EffectSpecIn>,
    #[schema(required = false, schema_with = nullable_crossfade_schema)]
    crossfade_ms: Option<u64>,
}

#[derive(Debug, Deserialize, ToSchema)]
struct CreatePresetRequest {
    #[schema(min_length = 1, max_length = 64)]
    id: String,
    #[schema(min_length = 1, max_length = 128)]
    name: String,
    #[schema(required = false, schema_with = nullable_string_schema)]
    description: Option<String>,
    #[serde(default)]
    #[schema(required = false)]
    effects: Vec<EffectSpecIn>,
    #[schema(required = false, schema_with = nullable_crossfade_schema)]
    crossfade_ms: Option<u64>,
}

#[derive(Debug, Clone, Default, PartialEq)]
enum Patch<T> {
    #[default]
    Missing,
    Null,
    Value(T),
}

impl<'de, T> Deserialize<'de> for Patch<T>
where
    T: Deserialize<'de>,
{
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        Ok(match Option::<T>::deserialize(deserializer)? {
            Some(value) => Self::Value(value),
            None => Self::Null,
        })
    }
}

pub(super) fn mode_write_router() -> OpenApiRouter<HttpState> {
    OpenApiRouter::default()
        .routes(routes!(create_mode))
        .routes(routes!(rename_mode))
        .routes(routes!(delete_mode))
        .routes(routes!(create_soundboard))
        .routes(routes!(delete_soundboard))
        .routes(routes!(add_soundboard_category))
        .routes(routes!(delete_soundboard_category))
        .routes(routes!(add_soundboard_item))
        .routes(routes!(update_soundboard_item))
        .routes(routes!(delete_soundboard_item))
        .routes(routes!(add_interrupt_template))
        .routes(routes!(update_interrupt_template))
        .routes(routes!(delete_interrupt_template))
        .routes(routes!(create_cue))
        .routes(routes!(update_cue))
        .routes(routes!(delete_cue))
        .routes(routes!(create_preset))
        .routes(routes!(update_preset))
        .routes(routes!(delete_preset))
}

#[utoipa::path(
    post,
    path = "/modes",
    request_body = CreateModeRequest,
    responses(
        (status = 201, description = "Successful Response", body = ModeSummary),
        (status = 422, description = "Validation Error", body = HttpValidationErrorBody)
    ),
    tag = "modes"
)]
async fn create_mode(
    State(state): State<HttpState>,
    headers: HeaderMap,
    payload: Result<Json<CreateModeRequest>, JsonRejection>,
) -> Result<(StatusCode, Json<ModeSummary>), ApiError> {
    authenticate(&state, &headers).await?;
    let Json(payload) = json_payload(payload)?;
    validate_slug(&payload.id, "mode")?;
    validate_text(&payload.name, 1, Some(128))?;
    let catalog = mode_catalog(&state)?;
    let mode_id = payload.id.clone();
    let result = commit(
        &state,
        ModeMutation::CreateMode {
            expected_generation: catalog.generation,
            manifest: ModeDocument {
                id: payload.id,
                name: payload.name,
                theme: None,
                panels: Vec::new(),
                playlist_categories: Vec::new(),
                interrupts: Vec::new(),
                integrations: IntegrationsDocument::default(),
                default_crossfade_ms: 0,
                default_soundboard: None,
                extra: BTreeMap::new(),
            },
        },
    )
    .await?;
    let mode = result
        .modes
        .get(&mode_id)
        .ok_or_else(ApiError::service_unavailable)?;
    Ok((StatusCode::CREATED, Json(summary(mode))))
}

#[utoipa::path(
    patch,
    path = "/modes/{mode_id}",
    params(("mode_id" = String, Path)),
    request_body = RenameModeRequest,
    responses(
        (status = 200, description = "Successful Response", body = ModeSummary),
        (status = 422, description = "Validation Error", body = HttpValidationErrorBody)
    ),
    tag = "modes"
)]
async fn rename_mode(
    State(state): State<HttpState>,
    headers: HeaderMap,
    Path(mode_id): Path<String>,
    payload: Result<Json<RenameModeRequest>, JsonRejection>,
) -> Result<Json<ModeSummary>, ApiError> {
    authenticate(&state, &headers).await?;
    let Json(payload) = json_payload(payload)?;
    validate_text(&payload.name, 1, Some(128))?;
    let catalog = mode_catalog(&state)?;
    let mut manifest = mode(&catalog, &mode_id)?.manifest.clone();
    manifest.name = payload.name;
    let result = commit(
        &state,
        ModeMutation::PutManifest {
            expected_generation: catalog.generation,
            mode_id: mode_id.clone(),
            manifest,
        },
    )
    .await?;
    Ok(Json(summary(mode(&result, &mode_id)?)))
}

#[utoipa::path(
    delete,
    path = "/modes/{mode_id}",
    params(("mode_id" = String, Path)),
    responses(
        (status = 204, description = "Successful Response"),
        (status = 422, description = "Validation Error", body = HttpValidationErrorBody)
    ),
    tag = "modes"
)]
async fn delete_mode(
    State(state): State<HttpState>,
    headers: HeaderMap,
    Path(mode_id): Path<String>,
) -> Result<StatusCode, ApiError> {
    authenticate(&state, &headers).await?;
    let catalog = mode_catalog(&state)?;
    commit(
        &state,
        ModeMutation::DeleteMode {
            expected_generation: catalog.generation,
            mode_id,
        },
    )
    .await?;
    Ok(StatusCode::NO_CONTENT)
}

#[utoipa::path(
    post,
    path = "/modes/{mode_id}/soundboards",
    params(("mode_id" = String, Path)),
    request_body = CreateSoundboardRequest,
    responses(
        (status = 201, description = "Successful Response", body = SoundboardManifest),
        (status = 422, description = "Validation Error", body = HttpValidationErrorBody)
    ),
    tag = "modes"
)]
async fn create_soundboard(
    State(state): State<HttpState>,
    headers: HeaderMap,
    Path(mode_id): Path<String>,
    payload: Result<Json<CreateSoundboardRequest>, JsonRejection>,
) -> Result<(StatusCode, Json<SoundboardManifest>), ApiError> {
    authenticate(&state, &headers).await?;
    let Json(payload) = json_payload(payload)?;
    validate_slug(&payload.id, "soundboard")?;
    validate_optional_text(payload.name.as_deref(), 0, Some(128))?;
    let catalog = mode_catalog(&state)?;
    mode(&catalog, &mode_id)?;
    let soundboard_id = payload.id;
    let result = commit(
        &state,
        ModeMutation::PutSoundboard {
            expected_generation: catalog.generation,
            mode_id: mode_id.clone(),
            soundboard_id: soundboard_id.clone(),
            document: SoundboardDocument {
                id: Some(soundboard_id.clone()),
                name: payload.name,
                categories: Vec::new(),
                extra: BTreeMap::new(),
            },
            create_only: true,
        },
    )
    .await?;
    Ok((
        StatusCode::CREATED,
        Json(soundboard_response(&result, &mode_id, &soundboard_id)?),
    ))
}

#[utoipa::path(
    delete,
    path = "/modes/{mode_id}/soundboards/{soundboard_id}",
    params(("mode_id" = String, Path), ("soundboard_id" = String, Path)),
    responses(
        (status = 204, description = "Successful Response"),
        (status = 422, description = "Validation Error", body = HttpValidationErrorBody)
    ),
    tag = "modes"
)]
async fn delete_soundboard(
    State(state): State<HttpState>,
    headers: HeaderMap,
    Path((mode_id, soundboard_id)): Path<(String, String)>,
) -> Result<StatusCode, ApiError> {
    authenticate(&state, &headers).await?;
    let catalog = mode_catalog(&state)?;
    commit(
        &state,
        ModeMutation::DeleteSoundboard {
            expected_generation: catalog.generation,
            mode_id,
            soundboard_id,
        },
    )
    .await?;
    Ok(StatusCode::NO_CONTENT)
}

#[utoipa::path(
    post,
    path = "/modes/{mode_id}/soundboards/{soundboard_id}/categories",
    params(("mode_id" = String, Path), ("soundboard_id" = String, Path)),
    request_body = AddCategoryRequest,
    responses(
        (status = 201, description = "Successful Response", body = SoundboardManifest),
        (status = 422, description = "Validation Error", body = HttpValidationErrorBody)
    ),
    tag = "modes"
)]
async fn add_soundboard_category(
    State(state): State<HttpState>,
    headers: HeaderMap,
    Path((mode_id, soundboard_id)): Path<(String, String)>,
    payload: Result<Json<AddCategoryRequest>, JsonRejection>,
) -> Result<(StatusCode, Json<SoundboardManifest>), ApiError> {
    authenticate(&state, &headers).await?;
    let Json(payload) = json_payload(payload)?;
    validate_slug(&payload.id, "category")?;
    validate_text(&payload.name, 1, Some(128))?;
    let catalog = mode_catalog(&state)?;
    let mut soundboard = soundboard(&catalog, &mode_id, &soundboard_id)?.clone();
    if soundboard
        .categories
        .iter()
        .any(|category| category.id == payload.id)
    {
        return Err(ApiError::conflict("category already exists"));
    }
    soundboard.categories.push(SoundboardCategoryDocument {
        id: payload.id,
        name: payload.name,
        items: Vec::new(),
        extra: BTreeMap::new(),
    });
    let result = put_soundboard(&state, &catalog, &mode_id, &soundboard_id, soundboard).await?;
    Ok((
        StatusCode::CREATED,
        Json(soundboard_response(&result, &mode_id, &soundboard_id)?),
    ))
}

#[utoipa::path(
    delete,
    path = "/modes/{mode_id}/soundboards/{soundboard_id}/categories/{category_id}",
    params(
        ("mode_id" = String, Path),
        ("soundboard_id" = String, Path),
        ("category_id" = String, Path)
    ),
    responses(
        (status = 200, description = "Successful Response", body = SoundboardManifest),
        (status = 422, description = "Validation Error", body = HttpValidationErrorBody)
    ),
    tag = "modes"
)]
async fn delete_soundboard_category(
    State(state): State<HttpState>,
    headers: HeaderMap,
    Path((mode_id, soundboard_id, category_id)): Path<(String, String, String)>,
) -> Result<Json<SoundboardManifest>, ApiError> {
    authenticate(&state, &headers).await?;
    let catalog = mode_catalog(&state)?;
    let mut soundboard = soundboard(&catalog, &mode_id, &soundboard_id)?.clone();
    let index = category_index(&soundboard, &category_id)?;
    soundboard.categories.remove(index);
    let result = put_soundboard(&state, &catalog, &mode_id, &soundboard_id, soundboard).await?;
    Ok(Json(soundboard_response(
        &result,
        &mode_id,
        &soundboard_id,
    )?))
}

#[utoipa::path(
    post,
    path = "/modes/{mode_id}/soundboards/{soundboard_id}/categories/{category_id}/items",
    params(
        ("mode_id" = String, Path),
        ("soundboard_id" = String, Path),
        ("category_id" = String, Path)
    ),
    request_body = AddItemRequest,
    responses(
        (status = 201, description = "Successful Response", body = SoundboardManifest),
        (status = 422, description = "Validation Error", body = HttpValidationErrorBody)
    ),
    tag = "modes"
)]
async fn add_soundboard_item(
    State(state): State<HttpState>,
    headers: HeaderMap,
    Path((mode_id, soundboard_id, category_id)): Path<(String, String, String)>,
    payload: Result<Json<AddItemRequest>, JsonRejection>,
) -> Result<(StatusCode, Json<SoundboardManifest>), ApiError> {
    authenticate(&state, &headers).await?;
    let Json(payload) = json_payload(payload)?;
    validate_item(
        &payload.file,
        &payload.name,
        payload.hotkey.as_deref(),
        payload.icon.as_deref(),
    )?;
    let catalog = mode_catalog(&state)?;
    let mut soundboard = soundboard(&catalog, &mode_id, &soundboard_id)?.clone();
    let index = category_index(&soundboard, &category_id)?;
    soundboard.categories[index]
        .items
        .push(SoundboardItemDocument {
            file: payload.file,
            name: payload.name,
            icon: nonempty(payload.icon),
            hotkey: nonempty(payload.hotkey),
            extra: BTreeMap::new(),
        });
    let result = put_soundboard(&state, &catalog, &mode_id, &soundboard_id, soundboard).await?;
    Ok((
        StatusCode::CREATED,
        Json(soundboard_response(&result, &mode_id, &soundboard_id)?),
    ))
}

#[utoipa::path(
    patch,
    path = "/modes/{mode_id}/soundboards/{soundboard_id}/categories/{category_id}/items/{index}",
    params(
        ("mode_id" = String, Path),
        ("soundboard_id" = String, Path),
        ("category_id" = String, Path),
        ("index" = i128, Path)
    ),
    request_body = UpdateItemRequest,
    responses(
        (status = 200, description = "Successful Response", body = SoundboardManifest),
        (status = 422, description = "Validation Error", body = HttpValidationErrorBody)
    ),
    tag = "modes"
)]
async fn update_soundboard_item(
    State(state): State<HttpState>,
    headers: HeaderMap,
    Path((mode_id, soundboard_id, category_id, index)): Path<(String, String, String, i64)>,
    payload: Result<Json<UpdateItemRequest>, JsonRejection>,
) -> Result<Json<SoundboardManifest>, ApiError> {
    authenticate(&state, &headers).await?;
    let Json(payload) = json_payload(payload)?;
    validate_item_patch(&payload)?;
    let catalog = mode_catalog(&state)?;
    let mut soundboard = soundboard(&catalog, &mode_id, &soundboard_id)?.clone();
    let category = category_index(&soundboard, &category_id)?;
    let item_index = bounded_index(index, soundboard.categories[category].items.len(), "item")?;
    apply_item_patch(
        &mut soundboard.categories[category].items[item_index],
        payload,
    )?;
    let result = put_soundboard(&state, &catalog, &mode_id, &soundboard_id, soundboard).await?;
    Ok(Json(soundboard_response(
        &result,
        &mode_id,
        &soundboard_id,
    )?))
}

#[utoipa::path(
    delete,
    path = "/modes/{mode_id}/soundboards/{soundboard_id}/categories/{category_id}/items/{index}",
    params(
        ("mode_id" = String, Path),
        ("soundboard_id" = String, Path),
        ("category_id" = String, Path),
        ("index" = i128, Path)
    ),
    responses(
        (status = 200, description = "Successful Response", body = SoundboardManifest),
        (status = 422, description = "Validation Error", body = HttpValidationErrorBody)
    ),
    tag = "modes"
)]
async fn delete_soundboard_item(
    State(state): State<HttpState>,
    headers: HeaderMap,
    Path((mode_id, soundboard_id, category_id, index)): Path<(String, String, String, i64)>,
) -> Result<Json<SoundboardManifest>, ApiError> {
    authenticate(&state, &headers).await?;
    let catalog = mode_catalog(&state)?;
    let mut soundboard = soundboard(&catalog, &mode_id, &soundboard_id)?.clone();
    let category = category_index(&soundboard, &category_id)?;
    let item_index = bounded_index(index, soundboard.categories[category].items.len(), "item")?;
    soundboard.categories[category].items.remove(item_index);
    let result = put_soundboard(&state, &catalog, &mode_id, &soundboard_id, soundboard).await?;
    Ok(Json(soundboard_response(
        &result,
        &mode_id,
        &soundboard_id,
    )?))
}

#[utoipa::path(
    post,
    path = "/modes/{mode_id}/interrupts",
    params(("mode_id" = String, Path)),
    request_body = InterruptTemplateCreate,
    responses(
        (status = 201, description = "Successful Response", body = [InterruptSpec]),
        (status = 422, description = "Validation Error", body = HttpValidationErrorBody)
    ),
    tag = "modes"
)]
async fn add_interrupt_template(
    State(state): State<HttpState>,
    headers: HeaderMap,
    Path(mode_id): Path<String>,
    payload: Result<Json<InterruptTemplateCreate>, JsonRejection>,
) -> Result<(StatusCode, Json<Vec<InterruptSpec>>), ApiError> {
    authenticate(&state, &headers).await?;
    let Json(payload) = json_payload(payload)?;
    validate_interrupt_create(&payload)?;
    let catalog = mode_catalog(&state)?;
    let mut manifest = mode(&catalog, &mode_id)?.manifest.clone();
    manifest.interrupts.push(InterruptDocument {
        name: payload.name,
        playlist: nonempty(payload.playlist),
        soundboard_item: nonempty(payload.soundboard_item),
        fade_in_ms: payload.fade_in_ms,
        fade_out_ms: payload.fade_out_ms,
        return_to_ambient: payload.return_to_ambient,
        duck_to: payload.duck_to,
        extra: BTreeMap::new(),
    });
    let result = put_manifest(&state, &catalog, &mode_id, manifest).await?;
    Ok((
        StatusCode::CREATED,
        Json(interrupts_response(&result, &mode_id)?),
    ))
}

#[utoipa::path(
    patch,
    path = "/modes/{mode_id}/interrupts/{index}",
    params(("mode_id" = String, Path), ("index" = i128, Path)),
    request_body = InterruptTemplateUpdate,
    responses(
        (status = 200, description = "Successful Response", body = [InterruptSpec]),
        (status = 422, description = "Validation Error", body = HttpValidationErrorBody)
    ),
    tag = "modes"
)]
async fn update_interrupt_template(
    State(state): State<HttpState>,
    headers: HeaderMap,
    Path((mode_id, index)): Path<(String, i64)>,
    payload: Result<Json<InterruptTemplateUpdate>, JsonRejection>,
) -> Result<Json<Vec<InterruptSpec>>, ApiError> {
    authenticate(&state, &headers).await?;
    let Json(payload) = json_payload(payload)?;
    validate_interrupt_patch(&payload)?;
    let catalog = mode_catalog(&state)?;
    let mut manifest = mode(&catalog, &mode_id)?.manifest.clone();
    let index = bounded_index(index, manifest.interrupts.len(), "interrupt")?;
    apply_interrupt_patch(&mut manifest.interrupts[index], payload)?;
    validate_interrupt_sources(
        manifest.interrupts[index].playlist.as_deref(),
        manifest.interrupts[index].soundboard_item.as_deref(),
    )?;
    let result = put_manifest(&state, &catalog, &mode_id, manifest).await?;
    Ok(Json(interrupts_response(&result, &mode_id)?))
}

#[utoipa::path(
    delete,
    path = "/modes/{mode_id}/interrupts/{index}",
    params(("mode_id" = String, Path), ("index" = i128, Path)),
    responses(
        (status = 200, description = "Successful Response", body = [InterruptSpec]),
        (status = 422, description = "Validation Error", body = HttpValidationErrorBody)
    ),
    tag = "modes"
)]
async fn delete_interrupt_template(
    State(state): State<HttpState>,
    headers: HeaderMap,
    Path((mode_id, index)): Path<(String, i64)>,
) -> Result<Json<Vec<InterruptSpec>>, ApiError> {
    authenticate(&state, &headers).await?;
    let catalog = mode_catalog(&state)?;
    let mut manifest = mode(&catalog, &mode_id)?.manifest.clone();
    let index = bounded_index(index, manifest.interrupts.len(), "interrupt")?;
    manifest.interrupts.remove(index);
    let result = put_manifest(&state, &catalog, &mode_id, manifest).await?;
    Ok(Json(interrupts_response(&result, &mode_id)?))
}

#[utoipa::path(
    post,
    path = "/modes/{mode_id}/cues",
    params(("mode_id" = String, Path)),
    request_body = CreateCueRequest,
    responses(
        (status = 201, description = "Successful Response", body = CueSpec),
        (status = 422, description = "Validation Error", body = HttpValidationErrorBody)
    ),
    tag = "modes"
)]
async fn create_cue(
    State(state): State<HttpState>,
    headers: HeaderMap,
    Path(mode_id): Path<String>,
    payload: Result<Json<CreateCueRequest>, JsonRejection>,
) -> Result<(StatusCode, Json<CueSpec>), ApiError> {
    authenticate(&state, &headers).await?;
    let Json(payload) = json_payload(payload)?;
    validate_slug(&payload.id, "cue")?;
    validate_cue_fields(
        &payload.name,
        payload.preset.as_deref(),
        payload.playlist.as_deref(),
        &payload.sfx,
        &payload.loops,
    )?;
    let catalog = mode_catalog(&state)?;
    mode(&catalog, &mode_id)?;
    let cue_id = payload.id;
    let document = cue_document(
        &cue_id,
        payload.name,
        payload.description,
        payload.preset,
        payload.playlist,
        payload.start_index,
        payload.start_ms,
        payload.sfx,
        payload.loops,
    );
    let result = commit(
        &state,
        ModeMutation::PutCue {
            expected_generation: catalog.generation,
            mode_id: mode_id.clone(),
            cue_id: cue_id.clone(),
            document,
            create_only: true,
        },
    )
    .await?;
    Ok((
        StatusCode::CREATED,
        Json(cue_response(&result, &mode_id, &cue_id)?),
    ))
}

#[utoipa::path(
    put,
    path = "/modes/{mode_id}/cues/{cue_id}",
    params(("mode_id" = String, Path), ("cue_id" = String, Path)),
    request_body = CueBodyRequest,
    responses(
        (status = 200, description = "Successful Response", body = CueSpec),
        (status = 422, description = "Validation Error", body = HttpValidationErrorBody)
    ),
    tag = "modes"
)]
async fn update_cue(
    State(state): State<HttpState>,
    headers: HeaderMap,
    Path((mode_id, cue_id)): Path<(String, String)>,
    payload: Result<Json<CueBodyRequest>, JsonRejection>,
) -> Result<Json<CueSpec>, ApiError> {
    authenticate(&state, &headers).await?;
    let Json(payload) = json_payload(payload)?;
    validate_slug(&cue_id, "cue")?;
    validate_cue_fields(
        &payload.name,
        payload.preset.as_deref(),
        payload.playlist.as_deref(),
        &payload.sfx,
        &payload.loops,
    )?;
    let catalog = mode_catalog(&state)?;
    let document = cue_document(
        &cue_id,
        payload.name,
        payload.description,
        payload.preset,
        payload.playlist,
        payload.start_index,
        payload.start_ms,
        payload.sfx,
        payload.loops,
    );
    let result = commit(
        &state,
        ModeMutation::PutCue {
            expected_generation: catalog.generation,
            mode_id: mode_id.clone(),
            cue_id: cue_id.clone(),
            document,
            create_only: false,
        },
    )
    .await?;
    Ok(Json(cue_response(&result, &mode_id, &cue_id)?))
}

#[utoipa::path(
    delete,
    path = "/modes/{mode_id}/cues/{cue_id}",
    params(("mode_id" = String, Path), ("cue_id" = String, Path)),
    responses(
        (status = 204, description = "Successful Response"),
        (status = 422, description = "Validation Error", body = HttpValidationErrorBody)
    ),
    tag = "modes"
)]
async fn delete_cue(
    State(state): State<HttpState>,
    headers: HeaderMap,
    Path((mode_id, cue_id)): Path<(String, String)>,
) -> Result<StatusCode, ApiError> {
    authenticate(&state, &headers).await?;
    let catalog = mode_catalog(&state)?;
    commit(
        &state,
        ModeMutation::DeleteCue {
            expected_generation: catalog.generation,
            mode_id,
            cue_id,
        },
    )
    .await?;
    Ok(StatusCode::NO_CONTENT)
}

#[utoipa::path(
    post,
    path = "/modes/{mode_id}/presets",
    params(("mode_id" = String, Path)),
    request_body = CreatePresetRequest,
    responses(
        (status = 201, description = "Successful Response", body = PresetManifest),
        (status = 422, description = "Validation Error", body = HttpValidationErrorBody)
    ),
    tag = "modes"
)]
async fn create_preset(
    State(state): State<HttpState>,
    headers: HeaderMap,
    Path(mode_id): Path<String>,
    payload: Result<Json<CreatePresetRequest>, JsonRejection>,
) -> Result<(StatusCode, Json<PresetManifest>), ApiError> {
    authenticate(&state, &headers).await?;
    let Json(payload) = json_payload(payload)?;
    validate_slug(&payload.id, "preset")?;
    validate_preset_fields(&payload.name, payload.crossfade_ms, &payload.effects)?;
    let catalog = mode_catalog(&state)?;
    mode(&catalog, &mode_id)?;
    let preset_id = payload.id;
    let result = commit(
        &state,
        ModeMutation::PutPreset {
            expected_generation: catalog.generation,
            mode_id: mode_id.clone(),
            preset_id: preset_id.clone(),
            document: preset_document(
                &preset_id,
                payload.name,
                payload.description,
                payload.effects,
                payload.crossfade_ms,
            ),
            create_only: true,
        },
    )
    .await?;
    Ok((
        StatusCode::CREATED,
        Json(preset_response(&result, &mode_id, &preset_id)?),
    ))
}

#[utoipa::path(
    put,
    path = "/modes/{mode_id}/presets/{preset_id}",
    params(("mode_id" = String, Path), ("preset_id" = String, Path)),
    request_body = PresetBodyRequest,
    responses(
        (status = 200, description = "Successful Response", body = PresetManifest),
        (status = 422, description = "Validation Error", body = HttpValidationErrorBody)
    ),
    tag = "modes"
)]
async fn update_preset(
    State(state): State<HttpState>,
    headers: HeaderMap,
    Path((mode_id, preset_id)): Path<(String, String)>,
    payload: Result<Json<PresetBodyRequest>, JsonRejection>,
) -> Result<Json<PresetManifest>, ApiError> {
    authenticate(&state, &headers).await?;
    let Json(payload) = json_payload(payload)?;
    validate_slug(&preset_id, "preset")?;
    validate_preset_fields(&payload.name, payload.crossfade_ms, &payload.effects)?;
    let catalog = mode_catalog(&state)?;
    let result = commit(
        &state,
        ModeMutation::PutPreset {
            expected_generation: catalog.generation,
            mode_id: mode_id.clone(),
            preset_id: preset_id.clone(),
            document: preset_document(
                &preset_id,
                payload.name,
                payload.description,
                payload.effects,
                payload.crossfade_ms,
            ),
            create_only: false,
        },
    )
    .await?;
    Ok(Json(preset_response(&result, &mode_id, &preset_id)?))
}

#[utoipa::path(
    delete,
    path = "/modes/{mode_id}/presets/{preset_id}",
    params(("mode_id" = String, Path), ("preset_id" = String, Path)),
    responses(
        (status = 204, description = "Successful Response"),
        (status = 422, description = "Validation Error", body = HttpValidationErrorBody)
    ),
    tag = "modes"
)]
async fn delete_preset(
    State(state): State<HttpState>,
    headers: HeaderMap,
    Path((mode_id, preset_id)): Path<(String, String)>,
) -> Result<StatusCode, ApiError> {
    authenticate(&state, &headers).await?;
    let catalog = mode_catalog(&state)?;
    commit(
        &state,
        ModeMutation::DeletePreset {
            expected_generation: catalog.generation,
            mode_id,
            preset_id,
        },
    )
    .await?;
    Ok(StatusCode::NO_CONTENT)
}

async fn authenticate(state: &HttpState, headers: &HeaderMap) -> Result<(), ApiError> {
    current_session(state, headers, SessionTouch::UpdateLastSeen)
        .await
        .map(|_| ())
}

fn json_payload<T>(payload: Result<Json<T>, JsonRejection>) -> Result<Json<T>, ApiError> {
    payload.map_err(|_| ApiError::validation())
}

async fn commit(state: &HttpState, mutation: ModeMutation) -> Result<Arc<ModeCatalog>, ApiError> {
    let coordinator = state
        .modes
        .as_ref()
        .ok_or_else(ApiError::service_unavailable)?;
    coordinator
        .mutate(mutation)
        .await
        .map(|report| report.catalog)
        .map_err(map_mutation_error)
}

fn map_mutation_error(error: ModeMutationError) -> ApiError {
    match error.kind {
        ModeMutationFailureKind::Invalid => ApiError::bad_request(error.code),
        ModeMutationFailureKind::NotFound => ApiError::plain_not_found(error.code),
        ModeMutationFailureKind::Conflict | ModeMutationFailureKind::Stale => {
            ApiError::conflict(error.code)
        }
        ModeMutationFailureKind::Unavailable => {
            tracing::error!(error = %error, "mode mutation failed");
            ApiError::service_unavailable()
        }
    }
}

fn mode<'a>(catalog: &'a ModeCatalog, mode_id: &str) -> Result<&'a super::ModeBundle, ApiError> {
    catalog
        .modes
        .get(mode_id)
        .ok_or_else(|| ApiError::plain_not_found("mode not loaded"))
}

fn soundboard<'a>(
    catalog: &'a ModeCatalog,
    mode_id: &str,
    soundboard_id: &str,
) -> Result<&'a SoundboardDocument, ApiError> {
    mode(catalog, mode_id)?
        .soundboards
        .get(soundboard_id)
        .ok_or_else(|| ApiError::plain_not_found("soundboard not found"))
}

async fn put_soundboard(
    state: &HttpState,
    catalog: &ModeCatalog,
    mode_id: &str,
    soundboard_id: &str,
    document: SoundboardDocument,
) -> Result<Arc<ModeCatalog>, ApiError> {
    commit(
        state,
        ModeMutation::PutSoundboard {
            expected_generation: catalog.generation,
            mode_id: mode_id.to_owned(),
            soundboard_id: soundboard_id.to_owned(),
            document,
            create_only: false,
        },
    )
    .await
}

async fn put_manifest(
    state: &HttpState,
    catalog: &ModeCatalog,
    mode_id: &str,
    manifest: ModeDocument,
) -> Result<Arc<ModeCatalog>, ApiError> {
    commit(
        state,
        ModeMutation::PutManifest {
            expected_generation: catalog.generation,
            mode_id: mode_id.to_owned(),
            manifest,
        },
    )
    .await
}

fn soundboard_response(
    catalog: &ModeCatalog,
    mode_id: &str,
    soundboard_id: &str,
) -> Result<SoundboardManifest, ApiError> {
    Ok(soundboard_manifest(
        soundboard_id,
        soundboard(catalog, mode_id, soundboard_id)?,
    ))
}

fn cue_response(catalog: &ModeCatalog, mode_id: &str, cue_id: &str) -> Result<CueSpec, ApiError> {
    let cue = mode(catalog, mode_id)?
        .cues
        .get(cue_id)
        .ok_or_else(ApiError::service_unavailable)?;
    Ok(cue_spec(cue_id, cue))
}

fn preset_response(
    catalog: &ModeCatalog,
    mode_id: &str,
    preset_id: &str,
) -> Result<PresetManifest, ApiError> {
    let preset = mode(catalog, mode_id)?
        .presets
        .get(preset_id)
        .ok_or_else(ApiError::service_unavailable)?;
    Ok(preset_manifest(preset_id, preset))
}

fn interrupts_response(
    catalog: &ModeCatalog,
    mode_id: &str,
) -> Result<Vec<InterruptSpec>, ApiError> {
    Ok(mode(catalog, mode_id)?
        .manifest
        .interrupts
        .iter()
        .map(interrupt_spec)
        .collect())
}

fn category_index(soundboard: &SoundboardDocument, category_id: &str) -> Result<usize, ApiError> {
    soundboard
        .categories
        .iter()
        .position(|category| category.id == category_id)
        .ok_or_else(|| ApiError::plain_not_found("category not found"))
}

fn bounded_index(index: i64, length: usize, kind: &'static str) -> Result<usize, ApiError> {
    let index = usize::try_from(index)
        .ok()
        .filter(|index| *index < length)
        .ok_or_else(|| {
            ApiError::plain_not_found(match kind {
                "item" => "item index out of range",
                _ => "interrupt index out of range",
            })
        })?;
    Ok(index)
}

fn validate_slug(value: &str, _kind: &'static str) -> Result<(), ApiError> {
    let mut characters = value.chars();
    let Some(first) = characters.next() else {
        return Err(ApiError::bad_request("invalid resource id"));
    };
    if value.chars().count() > 64
        || !(first.is_ascii_lowercase() || first.is_ascii_digit())
        || !characters.all(|character| {
            character.is_ascii_lowercase()
                || character.is_ascii_digit()
                || matches!(character, '-' | '_')
        })
    {
        return Err(ApiError::bad_request("invalid resource id"));
    }
    Ok(())
}

fn validate_text(value: &str, minimum: usize, maximum: Option<usize>) -> Result<(), ApiError> {
    let length = value.chars().count();
    if length < minimum || maximum.is_some_and(|maximum| length > maximum) {
        return Err(ApiError::validation());
    }
    Ok(())
}

fn validate_optional_text(
    value: Option<&str>,
    minimum: usize,
    maximum: Option<usize>,
) -> Result<(), ApiError> {
    if let Some(value) = value {
        validate_text(value, minimum, maximum)?;
    }
    Ok(())
}

fn validate_item(
    file: &str,
    name: &str,
    hotkey: Option<&str>,
    icon: Option<&str>,
) -> Result<(), ApiError> {
    validate_text(file, 1, None)?;
    validate_text(name, 1, Some(128))?;
    validate_optional_text(hotkey, 0, Some(8))?;
    validate_optional_text(icon, 0, Some(8))
}

fn validate_item_patch(payload: &UpdateItemRequest) -> Result<(), ApiError> {
    validate_patch_text(&payload.name, 0, Some(128))?;
    validate_patch_text(&payload.hotkey, 0, Some(8))?;
    validate_patch_text(&payload.icon, 0, Some(8))?;
    validate_patch_text(&payload.file, 0, None)
}

fn validate_patch_text(
    patch: &Patch<String>,
    minimum: usize,
    maximum: Option<usize>,
) -> Result<(), ApiError> {
    if let Patch::Value(value) = patch {
        validate_text(value, minimum, maximum)?;
    }
    Ok(())
}

fn apply_item_patch(
    item: &mut SoundboardItemDocument,
    payload: UpdateItemRequest,
) -> Result<(), ApiError> {
    apply_required_patch(&mut item.name, payload.name, "item name cannot be cleared")?;
    apply_optional_patch(&mut item.hotkey, payload.hotkey);
    apply_optional_patch(&mut item.icon, payload.icon);
    apply_required_patch(&mut item.file, payload.file, "item file cannot be cleared")
}

fn apply_required_patch(
    target: &mut String,
    patch: Patch<String>,
    error: &'static str,
) -> Result<(), ApiError> {
    match patch {
        Patch::Missing => Ok(()),
        Patch::Value(value) if !value.is_empty() => {
            *target = value;
            Ok(())
        }
        Patch::Null | Patch::Value(_) => Err(ApiError::bad_request(error)),
    }
}

fn apply_optional_patch(target: &mut Option<String>, patch: Patch<String>) {
    match patch {
        Patch::Missing => {}
        Patch::Null => *target = None,
        Patch::Value(value) => *target = nonempty(Some(value)),
    }
}

fn validate_interrupt_create(payload: &InterruptTemplateCreate) -> Result<(), ApiError> {
    validate_text(&payload.name, 1, Some(128))?;
    validate_optional_text(payload.playlist.as_deref(), 0, Some(256))?;
    validate_optional_text(payload.soundboard_item.as_deref(), 0, Some(512))?;
    validate_fade(payload.fade_in_ms)?;
    validate_fade(payload.fade_out_ms)?;
    if payload
        .duck_to
        .is_some_and(|value| !value.is_finite() || !(0.0..=1.0).contains(&value))
    {
        return Err(ApiError::validation());
    }
    validate_interrupt_sources(
        payload.playlist.as_deref(),
        payload.soundboard_item.as_deref(),
    )
}

fn validate_interrupt_patch(payload: &InterruptTemplateUpdate) -> Result<(), ApiError> {
    validate_patch_text(&payload.name, 0, Some(128))?;
    validate_patch_text(&payload.playlist, 0, Some(256))?;
    validate_patch_text(&payload.soundboard_item, 0, Some(512))?;
    for patch in [&payload.fade_in_ms, &payload.fade_out_ms] {
        if let Patch::Value(value) = patch {
            validate_fade(*value)?;
        }
    }
    if let Patch::Value(value) = payload.duck_to
        && (!value.is_finite() || !(0.0..=1.0).contains(&value))
    {
        return Err(ApiError::validation());
    }
    Ok(())
}

fn validate_fade(value: i64) -> Result<(), ApiError> {
    if (0..=10_000).contains(&value) {
        Ok(())
    } else {
        Err(ApiError::validation())
    }
}

fn validate_interrupt_sources(
    playlist: Option<&str>,
    soundboard_item: Option<&str>,
) -> Result<(), ApiError> {
    let has_playlist = playlist.is_some_and(|value| !value.is_empty());
    let has_item = soundboard_item.is_some_and(|value| !value.is_empty());
    if has_playlist == has_item {
        Err(ApiError::bad_request(
            "interrupt template must reference exactly one source",
        ))
    } else {
        Ok(())
    }
}

fn apply_interrupt_patch(
    interrupt: &mut InterruptDocument,
    payload: InterruptTemplateUpdate,
) -> Result<(), ApiError> {
    apply_required_patch(
        &mut interrupt.name,
        payload.name,
        "interrupt name cannot be cleared",
    )?;
    apply_optional_patch(&mut interrupt.playlist, payload.playlist);
    apply_optional_patch(&mut interrupt.soundboard_item, payload.soundboard_item);
    apply_default_patch(&mut interrupt.fade_in_ms, payload.fade_in_ms, 0);
    apply_default_patch(&mut interrupt.fade_out_ms, payload.fade_out_ms, 0);
    apply_default_patch(
        &mut interrupt.return_to_ambient,
        payload.return_to_ambient,
        true,
    );
    apply_nullable_copy_patch(&mut interrupt.duck_to, payload.duck_to);
    Ok(())
}

fn apply_default_patch<T>(target: &mut T, patch: Patch<T>, default: T) {
    match patch {
        Patch::Missing => {}
        Patch::Null => *target = default,
        Patch::Value(value) => *target = value,
    }
}

fn apply_nullable_copy_patch<T>(target: &mut Option<T>, patch: Patch<T>) {
    match patch {
        Patch::Missing => {}
        Patch::Null => *target = None,
        Patch::Value(value) => *target = Some(value),
    }
}

fn validate_cue_fields(
    name: &str,
    preset: Option<&str>,
    playlist: Option<&str>,
    sfx: &[CueSfxIn],
    loops: &[CueLoopIn],
) -> Result<(), ApiError> {
    validate_text(name, 1, Some(128))?;
    validate_optional_text(preset, 0, Some(64))?;
    validate_optional_text(playlist, 0, Some(256))?;
    for entry in sfx {
        validate_text(&entry.soundboard, 1, Some(128))?;
        validate_text(&entry.item, 1, Some(512))?;
        if !entry.volume.is_finite() || !(0.0..=1.0).contains(&entry.volume) {
            return Err(ApiError::validation());
        }
    }
    for entry in loops {
        validate_text(&entry.soundboard, 1, Some(128))?;
        validate_text(&entry.item, 1, Some(512))?;
        if !entry.interval_s.is_finite()
            || !(1.0..=3600.0).contains(&entry.interval_s)
            || !entry.volume.is_finite()
            || !(0.0..=1.0).contains(&entry.volume)
        {
            return Err(ApiError::validation());
        }
    }
    Ok(())
}

#[allow(clippy::too_many_arguments)]
fn cue_document(
    cue_id: &str,
    name: String,
    description: Option<String>,
    preset: Option<String>,
    playlist: Option<String>,
    start_index: u64,
    start_ms: u64,
    sfx: Vec<CueSfxIn>,
    loops: Vec<CueLoopIn>,
) -> CueDocument {
    CueDocument {
        id: Some(cue_id.to_owned()),
        name,
        description: nonempty(description),
        preset: nonempty(preset),
        playlist: nonempty(playlist),
        start_index,
        start_ms,
        sfx: sfx
            .into_iter()
            .map(|entry| CueSfxDocument {
                soundboard: entry.soundboard,
                item: entry.item,
                volume: entry.volume,
                extra: BTreeMap::new(),
            })
            .collect(),
        loops: loops
            .into_iter()
            .map(|entry| CueLoopDocument {
                soundboard: entry.soundboard,
                item: entry.item,
                interval_s: entry.interval_s,
                volume: entry.volume,
                extra: BTreeMap::new(),
            })
            .collect(),
        extra: BTreeMap::new(),
    }
}

fn validate_preset_fields(
    name: &str,
    crossfade_ms: Option<u64>,
    effects: &[EffectSpecIn],
) -> Result<(), ApiError> {
    validate_text(name, 1, Some(128))?;
    if crossfade_ms.is_some_and(|value| value > 60_000) {
        return Err(ApiError::validation());
    }
    if effects.iter().any(|effect| {
        effect.effect_type.is_empty()
            || !matches!(
                effect.effect_type.as_str(),
                "eq" | "reverb"
                    | "lowpass"
                    | "highpass"
                    | "bandpass"
                    | "delay"
                    | "distortion"
                    | "tremolo"
            )
    }) {
        return Err(ApiError::bad_request("unknown effect type"));
    }
    Ok(())
}

fn preset_document(
    preset_id: &str,
    name: String,
    description: Option<String>,
    effects: Vec<EffectSpecIn>,
    crossfade_ms: Option<u64>,
) -> PresetDocument {
    PresetDocument {
        id: Some(preset_id.to_owned()),
        name,
        description: nonempty(description),
        effects: effects
            .into_iter()
            .map(|effect| EffectDocument {
                effect_type: effect.effect_type,
                parameters: effect.parameters,
            })
            .collect(),
        crossfade_ms,
        extra: BTreeMap::new(),
    }
}

fn nonempty(value: Option<String>) -> Option<String> {
    value.filter(|value| !value.is_empty())
}

const fn default_true() -> bool {
    true
}

const fn default_volume() -> f64 {
    1.0
}

fn nullable_string_schema() -> RefOr<Schema> {
    nullable_text_schema(None, None)
}

fn nullable_nonempty_short_name_schema() -> RefOr<Schema> {
    nullable_text_schema(Some(1), Some(128))
}

fn nullable_short_name_schema() -> RefOr<Schema> {
    nullable_text_schema(None, Some(128))
}

fn nullable_tiny_string_schema() -> RefOr<Schema> {
    nullable_text_schema(None, Some(8))
}

fn nullable_slug_reference_schema() -> RefOr<Schema> {
    nullable_text_schema(None, Some(64))
}

fn nullable_playlist_schema() -> RefOr<Schema> {
    nullable_text_schema(None, Some(256))
}

fn nullable_item_reference_schema() -> RefOr<Schema> {
    nullable_text_schema(None, Some(512))
}

fn nullable_text_schema(minimum: Option<usize>, maximum: Option<usize>) -> RefOr<Schema> {
    let mut text = ObjectBuilder::new().schema_type(Type::String);
    if let Some(minimum) = minimum {
        text = text.min_length(Some(minimum));
    }
    if let Some(maximum) = maximum {
        text = text.max_length(Some(maximum));
    }
    Schema::AnyOf(
        AnyOfBuilder::new()
            .item(text)
            .item(ObjectBuilder::new().schema_type(Type::Null))
            .build(),
    )
    .into()
}

fn nullable_boolean_schema() -> RefOr<Schema> {
    Schema::AnyOf(
        AnyOfBuilder::new()
            .item(ObjectBuilder::new().schema_type(Type::Boolean))
            .item(ObjectBuilder::new().schema_type(Type::Null))
            .build(),
    )
    .into()
}

fn nullable_fade_schema() -> RefOr<Schema> {
    Schema::AnyOf(
        AnyOfBuilder::new()
            .item(
                ObjectBuilder::new()
                    .schema_type(Type::Integer)
                    .minimum(Some(0))
                    .maximum(Some(10_000)),
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
        .maximum(Some(10_000))
        .default(Some(serde_json::json!(0)))
        .into()
}

fn nullable_unit_interval_schema() -> RefOr<Schema> {
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

fn nullable_crossfade_schema() -> RefOr<Schema> {
    Schema::AnyOf(
        AnyOfBuilder::new()
            .item(
                ObjectBuilder::new()
                    .schema_type(Type::Integer)
                    .minimum(Some(0))
                    .maximum(Some(60_000)),
            )
            .item(ObjectBuilder::new().schema_type(Type::Null))
            .build(),
    )
    .into()
}
