use std::net::SocketAddr;
use std::sync::Arc;

use axum::Extension;
use axum::Json;
use axum::extract::ConnectInfo;
use axum::extract::rejection::JsonRejection;
use axum::extract::{Path, State};
use axum::http::header::{COOKIE, SET_COOKIE};
use axum::http::{HeaderMap, HeaderValue};
use axum::response::{IntoResponse, Response};
use music_application::auth::{
    ActiveSession, AuthService, AuthServiceConfig, AuthServiceError, AuthenticatedSession,
    LoginThrottle, LoginThrottleConfig, RevokeSessionOutcome, SecretSessionToken, SessionLookup,
    SessionTouch, SystemAuthClock, SystemSessionTokenSource, UnixSeconds, UserInfo,
};
use music_storage::{Argon2PasswordVerifier, SqliteStorage};
use serde::{Deserialize, Serialize};
use tokio::sync::Semaphore;
use utoipa::ToSchema;
use utoipa_axum::router::OpenApiRouter;
use utoipa_axum::routes;

use crate::config::AppConfig;
use crate::error::{ApiError, HttpValidationErrorBody, openapi_integer};
use crate::http::HttpState;

const ARGON2_CONCURRENCY: usize = 2;
const LOGIN_THROTTLE_DETAIL: &str = "too many login attempts; try again shortly";
const INVALID_CREDENTIALS_DETAIL: &str = "invalid credentials";
const NOT_AUTHENTICATED_DETAIL: &str = "not authenticated";

type RuntimeAuthService =
    AuthService<SqliteStorage, Argon2PasswordVerifier, SystemSessionTokenSource, SystemAuthClock>;

#[derive(Debug, Clone)]
pub(crate) struct CookieSettings {
    name: String,
    secure: bool,
    domain: Option<String>,
    max_age_seconds: u64,
}

impl CookieSettings {
    fn from_config(config: &AppConfig) -> Self {
        Self {
            name: config.session_cookie_name.clone(),
            secure: config.session_cookie_secure,
            domain: config.session_cookie_domain.clone(),
            max_age_seconds: u64::from(config.session_ttl_days).saturating_mul(86_400),
        }
    }

    fn set_header(&self, token: &SecretSessionToken) -> Result<HeaderValue, ApiError> {
        let mut value = format!(
            "{}={}; Max-Age={}; Path=/; HttpOnly; SameSite=Lax",
            self.name,
            token.expose_secret(),
            self.max_age_seconds
        );
        if self.secure {
            value.push_str("; Secure");
        }
        if let Some(domain) = &self.domain {
            value.push_str("; Domain=");
            value.push_str(domain);
        }
        HeaderValue::from_str(&value).map_err(|_| ApiError::internal())
    }

    fn delete_header(&self) -> Result<HeaderValue, ApiError> {
        let mut value = format!(
            "{}=; Max-Age=0; Expires=Thu, 01 Jan 1970 00:00:00 GMT; Path=/; HttpOnly; SameSite=Lax",
            self.name
        );
        if self.secure {
            value.push_str("; Secure");
        }
        if let Some(domain) = &self.domain {
            value.push_str("; Domain=");
            value.push_str(domain);
        }
        HeaderValue::from_str(&value).map_err(|_| ApiError::internal())
    }
}

#[derive(Debug)]
pub(crate) struct RuntimeAuth {
    service: RuntimeAuthService,
    throttle: LoginThrottle,
    password_slots: Arc<Semaphore>,
    cookies: CookieSettings,
}

impl RuntimeAuth {
    pub(crate) fn new(
        storage: Arc<SqliteStorage>,
        config: &AppConfig,
    ) -> Result<Self, AuthServiceError> {
        let service = AuthService::new(
            storage,
            Arc::new(Argon2PasswordVerifier),
            Arc::new(SystemSessionTokenSource),
            Arc::new(SystemAuthClock),
            AuthServiceConfig::new(config.session_ttl_days)?,
        );
        Ok(Self {
            service,
            throttle: LoginThrottle::new(LoginThrottleConfig::default()),
            password_slots: Arc::new(Semaphore::new(ARGON2_CONCURRENCY)),
            cookies: CookieSettings::from_config(config),
        })
    }

    async fn login(
        &self,
        throttle_key: &str,
        username: &str,
        password: &str,
    ) -> Result<AuthenticatedSession, LoginError> {
        if self.throttle.blocked(throttle_key) {
            return Err(LoginError::Throttled);
        }
        let _permit = Arc::clone(&self.password_slots)
            .try_acquire_owned()
            .map_err(|_| LoginError::Throttled)?;
        match self.service.login(username, password).await {
            Ok(session) => {
                self.throttle.record_success(throttle_key);
                Ok(session)
            }
            Err(AuthServiceError::InvalidCredentials) => {
                self.throttle.record_failure(throttle_key);
                Err(LoginError::InvalidCredentials)
            }
            Err(error) => Err(LoginError::Internal(error)),
        }
    }

    pub(crate) async fn authenticate(
        &self,
        token: &str,
        touch: SessionTouch,
    ) -> Result<SessionLookup, AuthServiceError> {
        self.service.authenticate(token, touch).await
    }
}

#[derive(Debug)]
enum LoginError {
    Throttled,
    InvalidCredentials,
    Internal(AuthServiceError),
}

#[derive(Debug, Clone)]
pub(crate) struct CurrentSession {
    pub(crate) user: UserInfo,
    pub(crate) token: SecretSessionToken,
    pub(crate) expires_at: UnixSeconds,
}

#[derive(Debug, Deserialize, ToSchema)]
struct LoginRequest {
    #[schema(min_length = 1, max_length = 64)]
    username: String,
    #[schema(min_length = 1, max_length = 256)]
    password: String,
}

#[derive(Debug, Serialize, ToSchema)]
struct UserInfoResponse {
    #[schema(schema_with = openapi_integer)]
    id: i64,
    username: String,
}

impl From<UserInfo> for UserInfoResponse {
    fn from(user: UserInfo) -> Self {
        Self {
            id: user.id,
            username: user.username,
        }
    }
}

#[derive(Debug, Serialize, ToSchema)]
struct ActiveSessionResponse {
    token_prefix: String,
    #[schema(format = DateTime)]
    created_at: String,
    #[schema(format = DateTime)]
    expires_at: String,
    #[schema(format = DateTime)]
    last_seen: String,
    is_current: bool,
}

impl TryFrom<ActiveSession> for ActiveSessionResponse {
    type Error = ApiError;

    fn try_from(session: ActiveSession) -> Result<Self, Self::Error> {
        Ok(Self {
            token_prefix: session.token_prefix,
            created_at: format_rfc3339(session.created_at)?,
            expires_at: format_rfc3339(session.expires_at)?,
            last_seen: format_rfc3339(session.last_seen)?,
            is_current: session.is_current,
        })
    }
}

pub(crate) fn auth_router() -> OpenApiRouter<HttpState> {
    OpenApiRouter::default()
        .routes(routes!(login))
        .routes(routes!(logout))
        .routes(routes!(me))
        .routes(routes!(list_sessions))
        .routes(routes!(revoke_session))
}

#[utoipa::path(
    post,
    path = "/auth/login",
    request_body = LoginRequest,
    responses(
        (status = 200, description = "Successful Response", body = UserInfoResponse),
        (status = 422, description = "Validation Error", body = HttpValidationErrorBody)
    ),
    tag = "auth"
)]
async fn login(
    State(state): State<HttpState>,
    peer: Option<Extension<ConnectInfo<SocketAddr>>>,
    payload: Result<Json<LoginRequest>, JsonRejection>,
) -> Result<Response, ApiError> {
    let Json(payload) = payload.map_err(|_| ApiError::validation())?;
    if !(1..=64).contains(&payload.username.chars().count())
        || !(1..=256).contains(&payload.password.chars().count())
    {
        return Err(ApiError::validation());
    }
    let auth = state.auth.ok_or_else(ApiError::service_unavailable)?;
    let throttle_key = peer.map_or_else(
        || "unknown".to_owned(),
        |Extension(ConnectInfo(address))| address.ip().to_string(),
    );
    let session = match auth
        .login(&throttle_key, &payload.username, &payload.password)
        .await
    {
        Ok(session) => session,
        Err(LoginError::Throttled) => {
            return Err(ApiError::too_many_requests(LOGIN_THROTTLE_DETAIL));
        }
        Err(LoginError::InvalidCredentials) => {
            return Err(ApiError::unauthorized(INVALID_CREDENTIALS_DETAIL));
        }
        Err(LoginError::Internal(error)) => {
            tracing::error!(error = %error, "login failed");
            return Err(ApiError::internal());
        }
    };
    let cookie = auth.cookies.set_header(&session.token)?;
    let mut response = Json(UserInfoResponse::from(session.user)).into_response();
    response.headers_mut().append(SET_COOKIE, cookie);
    Ok(response)
}

#[utoipa::path(
    post,
    path = "/auth/logout",
    responses(
        (status = 204, description = "Successful Response")
    ),
    tag = "auth"
)]
async fn logout(State(state): State<HttpState>, headers: HeaderMap) -> Result<Response, ApiError> {
    let current = current_session(&state, &headers, SessionTouch::UpdateLastSeen).await?;
    let auth = state.auth.ok_or_else(ApiError::service_unavailable)?;
    auth.service
        .logout(current.user.id)
        .await
        .map_err(|error| {
            tracing::error!(error = %error, "logout failed");
            ApiError::internal()
        })?;
    let mut response = axum::http::StatusCode::NO_CONTENT.into_response();
    response
        .headers_mut()
        .append(SET_COOKIE, auth.cookies.delete_header()?);
    Ok(response)
}

#[utoipa::path(
    get,
    path = "/auth/me",
    responses(
        (status = 200, description = "Successful Response", body = UserInfoResponse)
    ),
    tag = "auth"
)]
async fn me(
    State(state): State<HttpState>,
    headers: HeaderMap,
) -> Result<Json<UserInfoResponse>, ApiError> {
    let current = current_session(&state, &headers, SessionTouch::UpdateLastSeen).await?;
    Ok(Json(current.user.into()))
}

#[utoipa::path(
    get,
    path = "/auth/sessions",
    responses(
        (status = 200, description = "Successful Response", body = [ActiveSessionResponse])
    ),
    tag = "auth"
)]
async fn list_sessions(
    State(state): State<HttpState>,
    headers: HeaderMap,
) -> Result<Json<Vec<ActiveSessionResponse>>, ApiError> {
    let current = current_session(&state, &headers, SessionTouch::UpdateLastSeen).await?;
    let auth = state.auth.ok_or_else(ApiError::service_unavailable)?;
    let sessions = auth
        .service
        .list_sessions(current.user.id, current.token.expose_secret())
        .await
        .map_err(|error| {
            tracing::error!(error = %error, "session listing failed");
            ApiError::internal()
        })?;
    let sessions = sessions
        .into_iter()
        .map(ActiveSessionResponse::try_from)
        .collect::<Result<Vec<_>, _>>()?;
    Ok(Json(sessions))
}

#[utoipa::path(
    delete,
    path = "/auth/sessions/{token_prefix}",
    params(("token_prefix" = String, Path, description = "Unique session token prefix")),
    responses(
        (status = 204, description = "Successful Response"),
        (status = 422, description = "Validation Error", body = HttpValidationErrorBody)
    ),
    tag = "auth"
)]
async fn revoke_session(
    State(state): State<HttpState>,
    headers: HeaderMap,
    Path(token_prefix): Path<String>,
) -> Result<Response, ApiError> {
    let current = current_session(&state, &headers, SessionTouch::UpdateLastSeen).await?;
    let auth = state.auth.ok_or_else(ApiError::service_unavailable)?;
    match auth
        .service
        .revoke_session(current.user.id, &token_prefix)
        .await
    {
        Ok(RevokeSessionOutcome::Revoked) => Ok(axum::http::StatusCode::NO_CONTENT.into_response()),
        Ok(RevokeSessionOutcome::Missing) => Err(ApiError::plain_not_found("no matching session")),
        Ok(RevokeSessionOutcome::Ambiguous) => Err(ApiError::conflict(
            "prefix matches multiple sessions; pass a longer prefix",
        )),
        Err(AuthServiceError::TokenPrefixTooShort) => {
            Err(ApiError::bad_request("token prefix too short"))
        }
        Err(error) => {
            tracing::error!(error = %error, "session revocation failed");
            Err(ApiError::internal())
        }
    }
}

pub(crate) async fn current_session(
    state: &HttpState,
    headers: &HeaderMap,
    touch: SessionTouch,
) -> Result<CurrentSession, ApiError> {
    let auth = state
        .auth
        .as_ref()
        .ok_or_else(ApiError::service_unavailable)?;
    let token = session_cookie(headers, &auth.cookies.name)
        .ok_or_else(|| ApiError::unauthorized(NOT_AUTHENTICATED_DETAIL))?;
    match auth.authenticate(&token, touch).await {
        Ok(SessionLookup::Authenticated { user, expires_at }) => Ok(CurrentSession {
            user,
            token: SecretSessionToken::new(token),
            expires_at,
        }),
        Ok(SessionLookup::Expired | SessionLookup::Missing) => {
            Err(ApiError::unauthorized(NOT_AUTHENTICATED_DETAIL))
        }
        Err(error) => {
            tracing::error!(error = %error, "session lookup failed");
            Err(ApiError::internal())
        }
    }
}

pub(crate) async fn optional_session(
    state: &HttpState,
    headers: &HeaderMap,
    touch: SessionTouch,
) -> Result<Option<CurrentSession>, ApiError> {
    let Some(auth) = state.auth.as_ref() else {
        return Ok(None);
    };
    let Some(token) = session_cookie(headers, &auth.cookies.name) else {
        return Ok(None);
    };
    match auth.authenticate(&token, touch).await {
        Ok(SessionLookup::Authenticated { user, expires_at }) => Ok(Some(CurrentSession {
            user,
            token: SecretSessionToken::new(token),
            expires_at,
        })),
        Ok(SessionLookup::Expired | SessionLookup::Missing) => Ok(None),
        Err(error) => {
            tracing::error!(error = %error, "optional session lookup failed");
            Err(ApiError::internal())
        }
    }
}

pub(crate) fn session_cookie(headers: &HeaderMap, cookie_name: &str) -> Option<String> {
    headers.get_all(COOKIE).iter().find_map(|header| {
        let header = header.to_str().ok()?;
        header.split(';').find_map(|pair| {
            let (name, value) = pair.trim().split_once('=')?;
            (name == cookie_name && !value.is_empty()).then(|| value.to_owned())
        })
    })
}

fn format_rfc3339(timestamp: UnixSeconds) -> Result<String, ApiError> {
    let seconds = timestamp.get();
    let days = seconds.div_euclid(86_400);
    let seconds_in_day = seconds.rem_euclid(86_400);
    let (year, month, day) = civil_from_days(days);
    if !(0..=9_999).contains(&year) {
        return Err(ApiError::internal());
    }
    let hour = seconds_in_day / 3_600;
    let minute = (seconds_in_day % 3_600) / 60;
    let second = seconds_in_day % 60;
    Ok(format!(
        "{year:04}-{month:02}-{day:02}T{hour:02}:{minute:02}:{second:02}Z"
    ))
}

fn civil_from_days(days_since_epoch: i64) -> (i64, i64, i64) {
    let shifted = days_since_epoch + 719_468;
    let era = if shifted >= 0 {
        shifted
    } else {
        shifted - 146_096
    } / 146_097;
    let day_of_era = shifted - era * 146_097;
    let year_of_era =
        (day_of_era - day_of_era / 1_460 + day_of_era / 36_524 - day_of_era / 146_096) / 365;
    let mut year = year_of_era + era * 400;
    let day_of_year = day_of_era - (365 * year_of_era + year_of_era / 4 - year_of_era / 100);
    let month_piece = (5 * day_of_year + 2) / 153;
    let day = day_of_year - (153 * month_piece + 2) / 5 + 1;
    let month = month_piece + if month_piece < 10 { 3 } else { -9 };
    year += i64::from(month <= 2);
    (year, month, day)
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;
    use std::error::Error;
    use std::path::Path;

    use axum::body::{Body, to_bytes};
    use axum::http::header::{COOKIE, SET_COOKIE};
    use axum::http::{HeaderMap, HeaderValue};
    use axum::http::{Request, StatusCode};
    use axum::response::Response;
    use music_application::auth::UnixSeconds;
    use music_storage::{SqliteStorage, SqliteStorageOptions, hash_password};
    use serde_json::Value;
    use tempfile::tempdir;
    use tower::ServiceExt;

    use super::{format_rfc3339, session_cookie};
    use crate::{AppConfig, AppRuntime};

    #[test]
    fn parses_configured_cookie_without_accepting_a_name_prefix() {
        let mut headers = HeaderMap::new();
        headers.insert(
            axum::http::header::COOKIE,
            HeaderValue::from_static("music_session_old=nope; music_session=right-token"),
        );
        assert_eq!(
            session_cookie(&headers, "music_session").as_deref(),
            Some("right-token")
        );
    }

    #[test]
    fn formats_session_timestamps_as_utc_rfc3339() {
        assert_eq!(
            format_rfc3339(UnixSeconds::new(0)).ok().as_deref(),
            Some("1970-01-01T00:00:00Z")
        );
        assert_eq!(
            format_rfc3339(UnixSeconds::new(1_800_000_000))
                .ok()
                .as_deref(),
            Some("2027-01-15T08:00:00Z")
        );
    }

    fn runtime_config(root: &Path) -> Result<AppConfig, crate::ConfigError> {
        AppConfig::from_values(&BTreeMap::from([
            (
                "DATABASE_URL".to_owned(),
                format!("sqlite:///{}", root.join("app.db").display()),
            ),
            (
                "MUSIC_DIR".to_owned(),
                root.join("music").display().to_string(),
            ),
            (
                "SFX_LIBRARY_DIR".to_owned(),
                root.join("sfx").display().to_string(),
            ),
            (
                "MODES_DIR".to_owned(),
                root.join("modes").display().to_string(),
            ),
            (
                "DEVICES_FILE".to_owned(),
                root.join("devices.json").display().to_string(),
            ),
            (
                "STATIC_DIR".to_owned(),
                root.join("missing-static").display().to_string(),
            ),
            ("SESSION_COOKIE_SECURE".to_owned(), "false".to_owned()),
            ("SESSION_COOKIE_NAME".to_owned(), "test_session".to_owned()),
        ]))
    }

    async fn body_json(response: Response) -> Result<Value, Box<dyn Error>> {
        let body = to_bytes(response.into_body(), 1024 * 1024).await?;
        Ok(serde_json::from_slice(&body)?)
    }

    #[tokio::test]
    async fn auth_and_device_routes_share_the_configured_opaque_session()
    -> Result<(), Box<dyn Error>> {
        let directory = tempdir()?;
        let config = runtime_config(directory.path())?;
        let storage = SqliteStorage::open(SqliteStorageOptions::new(&config.database_path)).await?;
        let hash = hash_password("correct-password")?;
        storage
            .create_user("operator", &hash, UnixSeconds::new(1_800_000_000))
            .await?;
        storage.close().await;
        drop(storage);

        let runtime = AppRuntime::start(config).await?;
        let router = runtime.router()?;
        let invalid = router
            .clone()
            .oneshot(
                Request::post("/api/auth/login")
                    .header("content-type", "application/json")
                    .body(Body::from(r#"{"username":"","password":""}"#))?,
            )
            .await?;
        assert_eq!(invalid.status(), StatusCode::UNPROCESSABLE_ENTITY);

        let rejected = router
            .clone()
            .oneshot(
                Request::post("/api/auth/login")
                    .header("content-type", "application/json")
                    .body(Body::from(r#"{"username":"operator","password":"wrong"}"#))?,
            )
            .await?;
        assert_eq!(rejected.status(), StatusCode::UNAUTHORIZED);
        assert_eq!(body_json(rejected).await?["detail"], "invalid credentials");

        let login = router
            .clone()
            .oneshot(
                Request::post("/api/auth/login")
                    .header("content-type", "application/json")
                    .body(Body::from(
                        r#"{"username":"operator","password":"correct-password"}"#,
                    ))?,
            )
            .await?;
        assert_eq!(login.status(), StatusCode::OK);
        let set_cookie = login
            .headers()
            .get(SET_COOKIE)
            .and_then(|value| value.to_str().ok())
            .ok_or("missing session cookie")?
            .to_owned();
        assert!(set_cookie.starts_with("test_session="));
        assert!(set_cookie.contains("HttpOnly"));
        assert!(set_cookie.contains("SameSite=Lax"));
        assert!(!set_cookie.contains("; Secure"));
        let cookie = set_cookie
            .split(';')
            .next()
            .ok_or("missing cookie pair")?
            .to_owned();
        assert_eq!(body_json(login).await?["username"], "operator");

        let me = router
            .clone()
            .oneshot(
                Request::get("/api/auth/me")
                    .header(COOKIE, &cookie)
                    .body(Body::empty())?,
            )
            .await?;
        assert_eq!(me.status(), StatusCode::OK);

        let saved = router
            .clone()
            .oneshot(
                Request::put("/api/devices/offline-tv")
                    .header(COOKIE, &cookie)
                    .header("content-type", "application/json")
                    .body(Body::from(r#"{"name":"Living Room TV","is_output":true}"#))?,
            )
            .await?;
        assert_eq!(saved.status(), StatusCode::OK);
        let saved = body_json(saved).await?;
        assert_eq!(saved["connected"], false);
        assert_eq!(saved["is_output"], true);
        assert!(saved["added_at"].as_str().is_some());

        let sessions = router
            .clone()
            .oneshot(
                Request::get("/api/auth/sessions")
                    .header(COOKIE, &cookie)
                    .body(Body::empty())?,
            )
            .await?;
        let sessions = body_json(sessions).await?;
        assert_eq!(sessions.as_array().map(Vec::len), Some(1));
        assert_eq!(sessions[0]["is_current"], true);
        assert_eq!(sessions[0]["token_prefix"].as_str().map(str::len), Some(12));

        let logout = router
            .clone()
            .oneshot(
                Request::post("/api/auth/logout")
                    .header(COOKIE, &cookie)
                    .body(Body::empty())?,
            )
            .await?;
        assert_eq!(logout.status(), StatusCode::NO_CONTENT);
        assert!(
            logout.headers()[SET_COOKIE]
                .to_str()?
                .starts_with("test_session=; Max-Age=0")
        );
        let after = router
            .oneshot(
                Request::get("/api/auth/me")
                    .header(COOKIE, cookie)
                    .body(Body::empty())?,
            )
            .await?;
        assert_eq!(after.status(), StatusCode::UNAUTHORIZED);
        runtime.shutdown().await?;
        Ok(())
    }
}
