use std::any::Any;
use std::collections::BTreeMap;
use std::sync::Arc;
use std::time::Instant;

use axum::extract::{Query, Request, State};
use axum::http::header::{CACHE_CONTROL, CONTENT_TYPE};
use axum::http::{HeaderMap, HeaderName, HeaderValue, Method, StatusCode};
use axum::middleware::{self, Next};
use axum::response::{IntoResponse, Response};
use axum::routing::get;
use axum::{Json, Router};
use music_application::assistant::{AssistantService, LocalAnalysisService};
use music_application::jobs::JobService;
use music_application::modes::ModeCoordinatorHandle;
use music_application::playback::PlaybackActorHandle;
use music_application::playlists::PlaylistService;
use music_protocol::PlayerState;
use serde::Deserialize;
use tower::ServiceBuilder;
use tower_http::catch_panic::CatchPanicLayer;
use tower_http::cors::{AllowHeaders, CorsLayer};
use tower_http::limit::RequestBodyLimitLayer;
use tower_http::services::{ServeDir, ServeFile};
use tracing::Instrument;
use utoipa::openapi::RefOr;
use utoipa::openapi::schema::{AnyOfBuilder, ObjectBuilder, Schema, Type};
use utoipa::{IntoParams, PartialSchema, ToSchema};
use utoipa_axum::router::OpenApiRouter;
use utoipa_axum::routes;
use uuid::Uuid;

use crate::admin::BackupService;
use crate::auth::RuntimeAuth;
use crate::config::AppConfig;
use crate::devices::RuntimeDevices;
use crate::error::{ApiError, HttpValidationErrorBody, RuntimeError};
use crate::health::{ComponentStatus, HealthRegistry, ReadinessSnapshot};
use crate::library::RuntimeLibrary;
use crate::playback_projection::{canonical_state, guest_state};
use crate::provider_api::RuntimeProviders;
use crate::sfx::RuntimeSfx;

const X_REQUEST_ID: HeaderName = HeaderName::from_static("x-request-id");
const X_CONTENT_TYPE_OPTIONS: HeaderName = HeaderName::from_static("x-content-type-options");
const X_FRAME_OPTIONS: HeaderName = HeaderName::from_static("x-frame-options");
const REFERRER_POLICY: HeaderName = HeaderName::from_static("referrer-policy");
const PERMISSIONS_POLICY: HeaderName = HeaderName::from_static("permissions-policy");

#[derive(Debug, Clone)]
pub(crate) struct HttpState {
    pub(crate) health: HealthRegistry,
    pub(crate) assistant: Option<Arc<AssistantService>>,
    pub(crate) local_analysis: Option<Arc<LocalAnalysisService>>,
    pub(crate) playback: Option<PlaybackActorHandle>,
    pub(crate) auth: Option<Arc<RuntimeAuth>>,
    pub(crate) backup: Option<Arc<BackupService>>,
    pub(crate) devices: Option<Arc<RuntimeDevices>>,
    pub(crate) library: Option<Arc<RuntimeLibrary>>,
    pub(crate) jobs: Option<Arc<JobService>>,
    pub(crate) modes: Option<ModeCoordinatorHandle>,
    pub(crate) playlists: Option<Arc<PlaylistService>>,
    pub(crate) providers: Option<Arc<RuntimeProviders>>,
    pub(crate) sfx: Option<Arc<RuntimeSfx>>,
}

#[derive(Debug, Clone)]
pub(crate) struct RuntimeServices {
    pub(crate) health: HealthRegistry,
    pub(crate) assistant: Arc<AssistantService>,
    pub(crate) local_analysis: Arc<LocalAnalysisService>,
    pub(crate) playback: PlaybackActorHandle,
    pub(crate) auth: Arc<RuntimeAuth>,
    pub(crate) backup: Arc<BackupService>,
    pub(crate) devices: Arc<RuntimeDevices>,
    pub(crate) library: Arc<RuntimeLibrary>,
    pub(crate) jobs: Arc<JobService>,
    pub(crate) modes: ModeCoordinatorHandle,
    pub(crate) playlists: Arc<PlaylistService>,
    pub(crate) providers: Arc<RuntimeProviders>,
    pub(crate) sfx: Arc<RuntimeSfx>,
}

#[derive(Debug, Clone, Eq, PartialEq)]
pub struct CorrelationId(String);

impl CorrelationId {
    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

#[derive(utoipa::OpenApi)]
#[openapi(components(schemas(
    music_protocol::AmbientState,
    music_protocol::DeviceInfo,
    music_protocol::InterruptState,
    music_protocol::LoopingSfx,
    music_protocol::PositionReport,
    crate::modes::SoundboardManifest,
    crate::modes::CueSpec,
    crate::assistant::ModelTaggingScopeWire,
    crate::assistant::PlaylistAudioSignalResponse,
    crate::assistant::AudioSignalProfileResponse
)))]
struct MusicApi;

struct LivenessContract;

impl PartialSchema for LivenessContract {
    fn schema() -> RefOr<Schema> {
        ObjectBuilder::new()
            .additional_properties(Some(ObjectBuilder::new().schema_type(Type::String)))
            .build()
            .into()
    }
}

impl ToSchema for LivenessContract {}

pub fn build_router(config: &AppConfig, services: RuntimeServices) -> Result<Router, RuntimeError> {
    build_router_inner(
        config,
        HttpState {
            health: services.health,
            assistant: Some(services.assistant),
            local_analysis: Some(services.local_analysis),
            playback: Some(services.playback),
            auth: Some(services.auth),
            backup: Some(services.backup),
            devices: Some(services.devices),
            library: Some(services.library),
            jobs: Some(services.jobs),
            modes: Some(services.modes),
            playlists: Some(services.playlists),
            providers: Some(services.providers),
            sfx: Some(services.sfx),
        },
    )
}

#[cfg(test)]
fn build_router_without_playback(
    config: &AppConfig,
    health: HealthRegistry,
) -> Result<Router, RuntimeError> {
    build_router_inner(
        config,
        HttpState {
            health,
            assistant: None,
            local_analysis: None,
            playback: None,
            auth: None,
            backup: None,
            devices: None,
            library: None,
            jobs: None,
            modes: None,
            playlists: None,
            providers: None,
            sfx: None,
        },
    )
}

fn build_router_inner(config: &AppConfig, state: HttpState) -> Result<Router, RuntimeError> {
    let health = state.health.clone();
    let api = documented_api_router().with_state(state);
    let (mut router, _) = OpenApiRouter::with_openapi(<MusicApi as utoipa::OpenApi>::openapi())
        .nest("/api", api)
        .split_for_parts();

    let static_directory = config.static_dir.as_deref();
    let static_index = static_directory.map(|directory| directory.join("index.html"));
    if let (Some(static_directory), Some(static_index)) =
        (static_directory, static_index.filter(|path| path.is_file()))
        && static_directory.is_dir()
    {
        health.set_component("static_files", false, ComponentStatus::Ready);
        router = router.fallback_service(
            ServeDir::new(static_directory).fallback(ServeFile::new(static_index)),
        );
    } else {
        health.set_component("static_files", false, ComponentStatus::Degraded);
        router = router.fallback(root_not_found);
    }

    let cors = cors_layer(config)?;
    let middleware = ServiceBuilder::new()
        .layer(middleware::from_fn(request_context))
        .layer(cors)
        .layer(CatchPanicLayer::custom(panic_response))
        .layer(RequestBodyLimitLayer::new(config.request_body_limit_bytes));
    Ok(router.layer(middleware))
}

fn documented_api_router() -> OpenApiRouter<HttpState> {
    OpenApiRouter::default()
        .merge(crate::auth::auth_router())
        .merge(crate::assistant::assistant_router())
        .merge(crate::authoring::authoring_router())
        .merge(crate::admin::admin_router())
        .merge(crate::devices::device_router())
        .merge(crate::diagnostics::diagnostics_router())
        .merge(crate::library::library_router())
        .merge(crate::jobs::jobs_router())
        .merge(crate::modes::mode_router())
        .merge(crate::playlists::playlist_router())
        .merge(crate::provider_api::provider_router())
        .merge(crate::sfx::sfx_router())
        .merge(crate::cleanup::cleanup_router())
        .routes(routes!(liveness))
        .routes(routes!(readiness))
        .routes(routes!(sync_state))
        .route("/ws", get(crate::websocket::websocket_upgrade))
        .fallback(api_not_found)
}

pub(crate) fn openapi_document() -> utoipa::openapi::OpenApi {
    OpenApiRouter::<HttpState>::with_openapi(<MusicApi as utoipa::OpenApi>::openapi())
        .nest("/api", documented_api_router())
        .into_openapi()
}

fn cors_layer(config: &AppConfig) -> Result<CorsLayer, RuntimeError> {
    let origins = config
        .allowed_origins
        .iter()
        .map(|origin| {
            origin.parse::<HeaderValue>().map_err(|source| {
                RuntimeError::io(
                    "convert an allowed origin into an HTTP header",
                    std::io::Error::new(std::io::ErrorKind::InvalidInput, source),
                )
            })
        })
        .collect::<Result<Vec<_>, _>>()?;
    Ok(CorsLayer::new()
        .allow_credentials(true)
        .allow_methods([
            Method::GET,
            Method::POST,
            Method::PUT,
            Method::PATCH,
            Method::DELETE,
            Method::OPTIONS,
        ])
        .allow_headers(AllowHeaders::mirror_request())
        .allow_origin(origins))
}

#[utoipa::path(
    get,
    path = "/health",
    responses((status = 200, description = "Successful Response", body = LivenessContract))
)]
async fn liveness() -> Json<BTreeMap<&'static str, &'static str>> {
    Json(BTreeMap::from([("status", "ok")]))
}

#[utoipa::path(
    get,
    path = "/readiness",
    responses(
        (status = 200, description = "All critical components can accept traffic", body = ReadinessSnapshot),
        (status = 503, description = "A critical component is still starting or unavailable", body = ReadinessSnapshot)
    )
)]
async fn readiness(State(state): State<HttpState>) -> (StatusCode, Json<ReadinessSnapshot>) {
    let snapshot = state.health.snapshot();
    let status = if snapshot.accepts_traffic() {
        StatusCode::OK
    } else {
        StatusCode::SERVICE_UNAVAILABLE
    };
    (status, Json(snapshot))
}

#[derive(Debug, Default, Deserialize, IntoParams)]
#[into_params(parameter_in = Query)]
struct SyncStateQuery {
    #[param(schema_with = sync_client_id_schema)]
    client_id: Option<String>,
}

fn sync_client_id_schema() -> RefOr<Schema> {
    Schema::AnyOf(
        AnyOfBuilder::new()
            .item(
                ObjectBuilder::new()
                    .schema_type(Type::String)
                    .min_length(Some(1))
                    .max_length(Some(64))
                    .build(),
            )
            .item(ObjectBuilder::new().schema_type(Type::Null).build())
            .build(),
    )
    .into()
}

#[utoipa::path(
    get,
    path = "/sync/state",
    params(SyncStateQuery),
    responses(
        (status = 200, description = "Current canonical playback state", body = PlayerState),
        (status = 422, description = "Validation Error", body = HttpValidationErrorBody)
    )
)]
async fn sync_state(
    State(state): State<HttpState>,
    headers: HeaderMap,
    Query(query): Query<SyncStateQuery>,
) -> Result<Json<PlayerState>, ApiError> {
    if query
        .client_id
        .as_ref()
        .is_some_and(|client_id| !(1..=64).contains(&client_id.chars().count()))
    {
        return Err(ApiError::validation());
    }
    let playback = state
        .playback
        .as_ref()
        .ok_or_else(ApiError::service_unavailable)?;
    let publication = playback
        .snapshot()
        .await
        .map_err(|_| ApiError::service_unavailable())?;
    let canonical = canonical_state(&publication).map_err(|_| ApiError::internal())?;
    let authenticated = crate::auth::optional_session(
        &state,
        &headers,
        music_application::auth::SessionTouch::UpdateLastSeen,
    )
    .await?
    .is_some();
    Ok(Json(if authenticated {
        canonical
    } else {
        guest_state(canonical, query.client_id.as_deref())
    }))
}

async fn api_not_found() -> ApiError {
    ApiError::not_found()
}

async fn root_not_found() -> ApiError {
    ApiError::not_found()
}

fn panic_response(_: Box<dyn Any + Send + 'static>) -> Response {
    tracing::error!("HTTP handler panicked");
    ApiError::internal().into_response()
}

async fn request_context(mut request: Request, next: Next) -> Response {
    let correlation_id = CorrelationId(Uuid::new_v4().to_string());
    let request_id_header = HeaderValue::from_str(correlation_id.as_str()).ok();
    if let Some(header) = request_id_header.as_ref() {
        request.headers_mut().insert(X_REQUEST_ID, header.clone());
    }
    request.extensions_mut().insert(correlation_id.clone());

    let method = request.method().clone();
    let path = request.uri().path().to_owned();
    let started = Instant::now();
    let span = tracing::info_span!(
        "http.request",
        correlation_id = %correlation_id.as_str(),
        method = %method,
        path = %path,
    );
    async move {
        let mut response = next.run(request).await;
        if let Some(header) = request_id_header {
            response.headers_mut().insert(X_REQUEST_ID, header);
        }
        insert_if_missing(
            &mut response,
            X_CONTENT_TYPE_OPTIONS,
            HeaderValue::from_static("nosniff"),
        );
        insert_if_missing(
            &mut response,
            X_FRAME_OPTIONS,
            HeaderValue::from_static("SAMEORIGIN"),
        );
        insert_if_missing(
            &mut response,
            REFERRER_POLICY,
            HeaderValue::from_static("same-origin"),
        );
        insert_if_missing(
            &mut response,
            PERMISSIONS_POLICY,
            HeaderValue::from_static("camera=(), microphone=(), geolocation=()"),
        );
        apply_static_cache_policy(&path, &mut response);
        tracing::info!(
            status = response.status().as_u16(),
            elapsed_ms = started.elapsed().as_millis(),
            "HTTP request completed"
        );
        response
    }
    .instrument(span)
    .await
}

fn insert_if_missing(response: &mut Response, name: HeaderName, value: HeaderValue) {
    if !response.headers().contains_key(&name) {
        response.headers_mut().insert(name, value);
    }
}

fn apply_static_cache_policy(path: &str, response: &mut Response) {
    if path == "/api" || path.starts_with("/api/") {
        return;
    }
    let is_html = response
        .headers()
        .get(CONTENT_TYPE)
        .and_then(|value| value.to_str().ok())
        .is_some_and(|value| value.starts_with("text/html"));
    let value = if path.starts_with("/assets/") && !is_html && response.status().is_success() {
        HeaderValue::from_static("public, max-age=31536000, immutable")
    } else {
        HeaderValue::from_static("no-cache")
    };
    response.headers_mut().insert(CACHE_CONTROL, value);
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;
    use std::error::Error;
    use std::fs;

    use axum::body::{Body, to_bytes};
    use axum::http::header::{
        ACCESS_CONTROL_ALLOW_CREDENTIALS, ACCESS_CONTROL_ALLOW_ORIGIN, CACHE_CONTROL,
    };
    use axum::http::{HeaderValue, Method, Request, StatusCode};
    use axum::response::Response;
    use serde_json::Value;
    use tempfile::tempdir;
    use tower::ServiceExt;

    use super::build_router_without_playback as build_router;
    use crate::config::AppConfig;
    use crate::health::{ComponentStatus, HealthRegistry};

    fn test_config(values: &[(&str, String)]) -> Result<AppConfig, Box<dyn Error>> {
        let values = values
            .iter()
            .map(|(name, value)| ((*name).to_owned(), value.clone()))
            .collect::<BTreeMap<_, _>>();
        Ok(AppConfig::from_values(&values)?)
    }

    async fn body_json(response: Response) -> Result<Value, Box<dyn Error>> {
        let body = to_bytes(response.into_body(), 1024 * 1024).await?;
        Ok(serde_json::from_slice(&body)?)
    }

    async fn body_text(response: Response) -> Result<String, Box<dyn Error>> {
        let body = to_bytes(response.into_body(), 1024 * 1024).await?;
        Ok(String::from_utf8(body.to_vec())?)
    }

    #[tokio::test]
    async fn liveness_stays_compatible_while_readiness_is_explicit() -> Result<(), Box<dyn Error>> {
        let config = test_config(&[])?;
        let health = HealthRegistry::new();
        health.set_component("database", true, ComponentStatus::Ready);
        health.set_component("playback", true, ComponentStatus::Starting);
        let router = build_router(&config, health)?;

        let liveness = router
            .clone()
            .oneshot(
                Request::builder()
                    .uri("/api/health")
                    .header("x-request-id", "untrusted-client-value")
                    .body(Body::empty())?,
            )
            .await?;
        assert_eq!(liveness.status(), StatusCode::OK);
        assert_ne!(
            liveness.headers().get("x-request-id"),
            Some(&HeaderValue::from_static("untrusted-client-value"))
        );
        assert_eq!(
            liveness.headers().get("x-content-type-options"),
            Some(&HeaderValue::from_static("nosniff"))
        );
        assert_eq!(
            body_json(liveness).await?,
            serde_json::json!({"status":"ok"})
        );

        let readiness = router
            .oneshot(Request::get("/api/readiness").body(Body::empty())?)
            .await?;
        assert_eq!(readiness.status(), StatusCode::SERVICE_UNAVAILABLE);
        let body = body_json(readiness).await?;
        assert_eq!(body["status"], "starting");
        assert_eq!(body["components"]["database"], "ready");
        assert_eq!(body["components"]["playback"], "starting");
        Ok(())
    }

    #[tokio::test]
    async fn serves_the_spa_with_compatible_cache_policies() -> Result<(), Box<dyn Error>> {
        let directory = tempdir()?;
        let static_dir = directory.path().join("static");
        fs::create_dir_all(static_dir.join("assets"))?;
        fs::write(
            static_dir.join("index.html"),
            "<!doctype html><title>Music test shell</title>",
        )?;
        fs::write(static_dir.join("assets/app.js"), "window.musicLoaded=true;")?;
        let config = test_config(&[("STATIC_DIR", static_dir.display().to_string())])?;
        let router = build_router(&config, HealthRegistry::new())?;

        let root = router
            .clone()
            .oneshot(Request::get("/").body(Body::empty())?)
            .await?;
        assert_eq!(root.status(), StatusCode::OK);
        assert_eq!(root.headers()[CACHE_CONTROL], "no-cache");
        let root_text = body_text(root).await?;
        assert!(root_text.contains("Music test shell"));

        let client_route = router
            .clone()
            .oneshot(Request::get("/settings/playback").body(Body::empty())?)
            .await?;
        assert_eq!(client_route.headers()[CACHE_CONTROL], "no-cache");
        assert_eq!(body_text(client_route).await?, root_text);

        let asset = router
            .clone()
            .oneshot(Request::get("/assets/app.js").body(Body::empty())?)
            .await?;
        assert_eq!(
            asset.headers()[CACHE_CONTROL],
            "public, max-age=31536000, immutable"
        );
        assert_eq!(body_text(asset).await?, "window.musicLoaded=true;");

        let missing_api = router
            .oneshot(Request::get("/api/does-not-exist").body(Body::empty())?)
            .await?;
        assert_eq!(missing_api.status(), StatusCode::NOT_FOUND);
        assert_eq!(body_json(missing_api).await?["detail"]["code"], "not_found");
        Ok(())
    }

    #[tokio::test]
    async fn mirrors_allowed_preflight_headers_and_bounds_request_bodies()
    -> Result<(), Box<dyn Error>> {
        let config = test_config(&[
            ("MAX_UPLOAD_FILES", "1".to_owned()),
            ("MAX_UPLOAD_FILE_BYTES", "4".to_owned()),
        ])?;
        let oversized_content_length = config.request_body_limit_bytes + 1;
        let router = build_router(&config, HealthRegistry::new())?;

        let preflight = router
            .clone()
            .oneshot(
                Request::builder()
                    .method(Method::OPTIONS)
                    .uri("/api/health")
                    .header("origin", "http://localhost:5173")
                    .header("access-control-request-method", "GET")
                    .header("access-control-request-headers", "x-example")
                    .body(Body::empty())?,
            )
            .await?;
        assert_eq!(
            preflight.headers().get(ACCESS_CONTROL_ALLOW_ORIGIN),
            Some(&HeaderValue::from_static("http://localhost:5173"))
        );
        assert_eq!(
            preflight.headers().get(ACCESS_CONTROL_ALLOW_CREDENTIALS),
            Some(&HeaderValue::from_static("true"))
        );

        let too_large = router
            .oneshot(
                Request::post("/api/does-not-exist")
                    .header("content-length", oversized_content_length.to_string())
                    .body(Body::from("x"))?,
            )
            .await?;
        assert_eq!(too_large.status(), StatusCode::PAYLOAD_TOO_LARGE);
        Ok(())
    }
}
