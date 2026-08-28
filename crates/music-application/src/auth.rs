use std::collections::HashMap;
use std::error::Error;
use std::fmt::{self, Debug, Display, Formatter};
use std::future::Future;
use std::pin::Pin;
use std::sync::{Arc, Mutex, MutexGuard};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use base64::Engine;
use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use rand::TryRngCore;
use rand::rngs::OsRng;
use zeroize::Zeroizing;

const SESSION_TOKEN_BYTES: usize = 48;
const SESSION_PREFIX_LENGTH: usize = 12;
const MINIMUM_REVOKE_PREFIX_LENGTH: usize = 8;
const SECONDS_PER_DAY: i64 = 86_400;

pub type DependencyError = Box<dyn Error + Send + Sync + 'static>;
pub type AuthFuture<'a, T> = Pin<Box<dyn Future<Output = Result<T, DependencyError>> + Send + 'a>>;

#[derive(Debug, Clone, Copy, Eq, PartialEq, Ord, PartialOrd)]
pub struct UnixSeconds(i64);

impl UnixSeconds {
    #[must_use]
    pub const fn new(value: i64) -> Self {
        Self(value)
    }

    #[must_use]
    pub const fn get(self) -> i64 {
        self.0
    }

    pub fn checked_add_days(self, days: u32) -> Result<Self, AuthServiceError> {
        let seconds = i64::from(days)
            .checked_mul(SECONDS_PER_DAY)
            .ok_or(AuthServiceError::TimestampOverflow)?;
        self.0
            .checked_add(seconds)
            .map(Self)
            .ok_or(AuthServiceError::TimestampOverflow)
    }
}

#[derive(Clone, Eq, PartialEq)]
pub struct SecretSessionToken(Zeroizing<String>);

impl SecretSessionToken {
    #[must_use]
    pub fn new(value: impl Into<String>) -> Self {
        Self(Zeroizing::new(value.into()))
    }

    #[must_use]
    pub fn expose_secret(&self) -> &str {
        self.0.as_str()
    }
}

impl Debug for SecretSessionToken {
    fn fmt(&self, formatter: &mut Formatter<'_>) -> fmt::Result {
        formatter.write_str("SecretSessionToken([REDACTED])")
    }
}

#[derive(Clone, Eq, PartialEq)]
pub struct PasswordHash(String);

impl PasswordHash {
    #[must_use]
    pub fn new(value: impl Into<String>) -> Self {
        Self(value.into())
    }

    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl Debug for PasswordHash {
    fn fmt(&self, formatter: &mut Formatter<'_>) -> fmt::Result {
        formatter.write_str("PasswordHash([REDACTED])")
    }
}

#[derive(Debug, Clone, Eq, PartialEq)]
pub struct UserInfo {
    pub id: i64,
    pub username: String,
}

#[derive(Debug, Clone, Eq, PartialEq)]
pub struct UserCredentialRecord {
    pub user: UserInfo,
    pub password_hash: PasswordHash,
}

#[derive(Debug, Clone, Eq, PartialEq)]
pub struct AuthenticatedSession {
    pub user: UserInfo,
    pub token: SecretSessionToken,
    pub expires_at: UnixSeconds,
}

#[derive(Debug, Clone, Eq, PartialEq)]
pub struct StoredSessionSummary {
    pub token: SecretSessionToken,
    pub created_at: UnixSeconds,
    pub expires_at: UnixSeconds,
    pub last_seen: UnixSeconds,
}

#[derive(Debug, Clone, Eq, PartialEq)]
pub struct ActiveSession {
    pub token_prefix: String,
    pub created_at: UnixSeconds,
    pub expires_at: UnixSeconds,
    pub last_seen: UnixSeconds,
    pub is_current: bool,
}

#[derive(Debug, Clone, Eq, PartialEq)]
pub enum SessionLookup {
    Authenticated {
        user: UserInfo,
        expires_at: UnixSeconds,
    },
    Expired,
    Missing,
}

#[derive(Debug, Clone, Copy, Eq, PartialEq)]
pub enum SessionTouch {
    UpdateLastSeen,
    PreserveLastSeen,
}

#[derive(Debug, Clone, Copy, Eq, PartialEq)]
pub enum RevokeSessionOutcome {
    Revoked,
    Missing,
    Ambiguous,
}

pub trait AuthRepository: Send + Sync + 'static {
    fn find_user_by_username<'a>(
        &'a self,
        username: &'a str,
    ) -> AuthFuture<'a, Option<UserCredentialRecord>>;

    fn create_session<'a>(
        &'a self,
        user_id: i64,
        token: &'a str,
        created_at: UnixSeconds,
        expires_at: UnixSeconds,
    ) -> AuthFuture<'a, ()>;

    fn lookup_session<'a>(
        &'a self,
        token: &'a str,
        now: UnixSeconds,
        touch: SessionTouch,
        last_seen_throttle: Duration,
    ) -> AuthFuture<'a, SessionLookup>;

    fn delete_sessions_for_user(&self, user_id: i64) -> AuthFuture<'_, u64>;

    fn list_sessions(&self, user_id: i64) -> AuthFuture<'_, Vec<StoredSessionSummary>>;

    fn revoke_session_prefix<'a>(
        &'a self,
        user_id: i64,
        token_prefix: &'a str,
    ) -> AuthFuture<'a, RevokeSessionOutcome>;
}

pub trait PasswordVerifier: Send + Sync + 'static {
    /// Verify the real hash when present, or an equally expensive dummy hash
    /// for an unknown username.
    fn verify_candidate(
        &self,
        password_hash: Option<&str>,
        candidate: &str,
    ) -> Result<bool, DependencyError>;
}

pub trait SessionTokenSource: Send + Sync + 'static {
    fn generate(&self) -> Result<SecretSessionToken, DependencyError>;
}

pub trait AuthClock: Send + Sync + 'static {
    fn now(&self) -> Result<UnixSeconds, DependencyError>;
}

#[derive(Debug, Clone, Copy, Eq, PartialEq)]
pub struct AuthServiceConfig {
    pub session_ttl_days: u32,
    pub last_seen_throttle: Duration,
}

impl AuthServiceConfig {
    pub fn new(session_ttl_days: u32) -> Result<Self, AuthServiceError> {
        if session_ttl_days == 0 {
            return Err(AuthServiceError::InvalidConfiguration(
                "session TTL must be positive",
            ));
        }
        Ok(Self {
            session_ttl_days,
            last_seen_throttle: Duration::from_secs(60),
        })
    }
}

#[derive(Debug)]
pub struct AuthService<R, V, T, C> {
    repository: Arc<R>,
    verifier: Arc<V>,
    token_source: Arc<T>,
    clock: Arc<C>,
    config: AuthServiceConfig,
}

impl<R, V, T, C> Clone for AuthService<R, V, T, C> {
    fn clone(&self) -> Self {
        Self {
            repository: Arc::clone(&self.repository),
            verifier: Arc::clone(&self.verifier),
            token_source: Arc::clone(&self.token_source),
            clock: Arc::clone(&self.clock),
            config: self.config,
        }
    }
}

impl<R, V, T, C> AuthService<R, V, T, C>
where
    R: AuthRepository,
    V: PasswordVerifier,
    T: SessionTokenSource,
    C: AuthClock,
{
    #[must_use]
    pub fn new(
        repository: Arc<R>,
        verifier: Arc<V>,
        token_source: Arc<T>,
        clock: Arc<C>,
        config: AuthServiceConfig,
    ) -> Self {
        Self {
            repository,
            verifier,
            token_source,
            clock,
            config,
        }
    }

    pub async fn login(
        &self,
        username: &str,
        password: &str,
    ) -> Result<AuthenticatedSession, AuthServiceError> {
        let record = self.verified_user(username, password).await?;

        let now = self
            .clock
            .now()
            .map_err(|source| AuthServiceError::dependency("system clock", source))?;
        let expires_at = now.checked_add_days(self.config.session_ttl_days)?;
        let token = self
            .token_source
            .generate()
            .map_err(|source| AuthServiceError::dependency("session token generation", source))?;
        self.repository
            .create_session(record.user.id, token.expose_secret(), now, expires_at)
            .await
            .map_err(|source| AuthServiceError::dependency("session creation", source))?;
        Ok(AuthenticatedSession {
            user: record.user,
            token,
            expires_at,
        })
    }

    pub async fn verify_credentials(
        &self,
        username: &str,
        password: &str,
    ) -> Result<(), AuthServiceError> {
        self.verified_user(username, password).await.map(|_| ())
    }

    async fn verified_user(
        &self,
        username: &str,
        password: &str,
    ) -> Result<UserCredentialRecord, AuthServiceError> {
        if !(1..=64).contains(&username.chars().count())
            || !(1..=256).contains(&password.chars().count())
        {
            return Err(AuthServiceError::InvalidCredentials);
        }
        let record = self
            .repository
            .find_user_by_username(username)
            .await
            .map_err(|source| AuthServiceError::dependency("user lookup", source))?;
        let password_hash = record
            .as_ref()
            .map(|record| record.password_hash.as_str().to_owned());
        let candidate = password.to_owned();
        let verifier = Arc::clone(&self.verifier);
        let verified = tokio::task::spawn_blocking(move || {
            verifier.verify_candidate(password_hash.as_deref(), &candidate)
        })
        .await
        .map_err(|source| AuthServiceError::dependency("password worker", Box::new(source)))?
        .map_err(|source| AuthServiceError::dependency("password verification", source))?;
        let Some(record) = record.filter(|_| verified) else {
            return Err(AuthServiceError::InvalidCredentials);
        };
        Ok(record)
    }

    pub async fn authenticate(
        &self,
        token: &str,
        touch: SessionTouch,
    ) -> Result<SessionLookup, AuthServiceError> {
        if token.is_empty() || token.len() > 96 {
            return Ok(SessionLookup::Missing);
        }
        let now = self
            .clock
            .now()
            .map_err(|source| AuthServiceError::dependency("system clock", source))?;
        self.repository
            .lookup_session(token, now, touch, self.config.last_seen_throttle)
            .await
            .map_err(|source| AuthServiceError::dependency("session lookup", source))
    }

    pub async fn logout(&self, user_id: i64) -> Result<(), AuthServiceError> {
        self.repository
            .delete_sessions_for_user(user_id)
            .await
            .map(|_| ())
            .map_err(|source| AuthServiceError::dependency("session logout", source))
    }

    pub async fn list_sessions(
        &self,
        user_id: i64,
        current_token: &str,
    ) -> Result<Vec<ActiveSession>, AuthServiceError> {
        self.repository
            .list_sessions(user_id)
            .await
            .map_err(|source| AuthServiceError::dependency("session listing", source))
            .map(|sessions| {
                sessions
                    .into_iter()
                    .map(|session| ActiveSession {
                        token_prefix: session
                            .token
                            .expose_secret()
                            .chars()
                            .take(SESSION_PREFIX_LENGTH)
                            .collect(),
                        created_at: session.created_at,
                        expires_at: session.expires_at,
                        last_seen: session.last_seen,
                        is_current: session.token.expose_secret() == current_token,
                    })
                    .collect()
            })
    }

    pub async fn revoke_session(
        &self,
        user_id: i64,
        token_prefix: &str,
    ) -> Result<RevokeSessionOutcome, AuthServiceError> {
        if token_prefix.chars().count() < MINIMUM_REVOKE_PREFIX_LENGTH {
            return Err(AuthServiceError::TokenPrefixTooShort);
        }
        self.repository
            .revoke_session_prefix(user_id, token_prefix)
            .await
            .map_err(|source| AuthServiceError::dependency("session revocation", source))
    }
}

#[derive(Debug, Default)]
pub struct SystemSessionTokenSource;

impl SessionTokenSource for SystemSessionTokenSource {
    fn generate(&self) -> Result<SecretSessionToken, DependencyError> {
        let mut bytes = [0_u8; SESSION_TOKEN_BYTES];
        OsRng
            .try_fill_bytes(&mut bytes)
            .map_err(|source| -> DependencyError { Box::new(source) })?;
        Ok(SecretSessionToken::new(URL_SAFE_NO_PAD.encode(bytes)))
    }
}

#[derive(Debug, Default)]
pub struct SystemAuthClock;

impl AuthClock for SystemAuthClock {
    fn now(&self) -> Result<UnixSeconds, DependencyError> {
        let duration = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map_err(|source| -> DependencyError { Box::new(source) })?;
        let seconds = i64::try_from(duration.as_secs())
            .map_err(|source| -> DependencyError { Box::new(source) })?;
        Ok(UnixSeconds::new(seconds))
    }
}

#[derive(Debug)]
pub enum AuthServiceError {
    InvalidConfiguration(&'static str),
    InvalidCredentials,
    TokenPrefixTooShort,
    TimestampOverflow,
    Dependency {
        operation: &'static str,
        source: DependencyError,
    },
}

impl AuthServiceError {
    fn dependency(operation: &'static str, source: DependencyError) -> Self {
        Self::Dependency { operation, source }
    }
}

impl Display for AuthServiceError {
    fn fmt(&self, formatter: &mut Formatter<'_>) -> fmt::Result {
        match self {
            Self::InvalidConfiguration(detail) => {
                write!(formatter, "invalid authentication configuration: {detail}")
            }
            Self::InvalidCredentials => formatter.write_str("invalid credentials"),
            Self::TokenPrefixTooShort => formatter.write_str("token prefix too short"),
            Self::TimestampOverflow => formatter.write_str("session timestamp overflowed"),
            Self::Dependency { operation, .. } => {
                write!(
                    formatter,
                    "authentication dependency failed during {operation}"
                )
            }
        }
    }
}

impl Error for AuthServiceError {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        match self {
            Self::Dependency { source, .. } => Some(source.as_ref()),
            _ => None,
        }
    }
}

#[derive(Debug, Clone, Copy)]
pub struct LoginThrottleConfig {
    pub window: Duration,
    pub per_key_failures: usize,
    pub global_failures: usize,
    pub maximum_keys: usize,
}

impl Default for LoginThrottleConfig {
    fn default() -> Self {
        Self {
            window: Duration::from_secs(60),
            per_key_failures: 10,
            global_failures: 50,
            maximum_keys: 1_024,
        }
    }
}

#[derive(Debug)]
pub struct LoginThrottle {
    config: LoginThrottleConfig,
    failures: Mutex<HashMap<String, Vec<Instant>>>,
}

impl LoginThrottle {
    #[must_use]
    pub fn new(config: LoginThrottleConfig) -> Self {
        Self {
            config,
            failures: Mutex::new(HashMap::new()),
        }
    }

    #[must_use]
    pub fn blocked(&self, key: &str) -> bool {
        self.blocked_at(key, Instant::now())
    }

    pub fn record_failure(&self, key: &str) {
        self.record_failure_at(key, Instant::now());
    }

    pub fn record_success(&self, key: &str) {
        self.failures().remove(key);
    }

    fn blocked_at(&self, key: &str, now: Instant) -> bool {
        let mut failures = self.failures();
        retain_recent(&mut failures, now, self.config.window);
        let per_key = failures.get(key).map_or(0, Vec::len);
        let global = failures.values().map(Vec::len).sum::<usize>();
        per_key >= self.config.per_key_failures || global >= self.config.global_failures
    }

    fn record_failure_at(&self, key: &str, now: Instant) {
        let mut failures = self.failures();
        retain_recent(&mut failures, now, self.config.window);
        if !failures.contains_key(key) && failures.len() >= self.config.maximum_keys {
            return;
        }
        failures.entry(key.to_owned()).or_default().push(now);
    }

    fn failures(&self) -> MutexGuard<'_, HashMap<String, Vec<Instant>>> {
        match self.failures.lock() {
            Ok(guard) => guard,
            Err(poisoned) => poisoned.into_inner(),
        }
    }
}

fn retain_recent(failures: &mut HashMap<String, Vec<Instant>>, now: Instant, window: Duration) {
    failures.retain(|_, attempts| {
        attempts.retain(|attempt| now.saturating_duration_since(*attempt) < window);
        !attempts.is_empty()
    });
}

#[cfg(test)]
mod tests {
    use std::time::{Duration, Instant};

    use super::{
        LoginThrottle, LoginThrottleConfig, SecretSessionToken, SessionTokenSource,
        SystemSessionTokenSource,
    };

    #[test]
    fn session_tokens_are_url_safe_unique_and_redacted() -> Result<(), super::DependencyError> {
        let source = SystemSessionTokenSource;
        let first = source.generate()?;
        let second = source.generate()?;
        assert_eq!(first.expose_secret().len(), 64);
        assert!(
            first
                .expose_secret()
                .bytes()
                .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_'))
        );
        assert_ne!(first, second);
        assert_eq!(format!("{first:?}"), "SecretSessionToken([REDACTED])");
        Ok(())
    }

    #[test]
    fn throttle_bounds_each_client_and_global_hash_work() {
        let throttle = LoginThrottle::new(LoginThrottleConfig {
            window: Duration::from_secs(60),
            per_key_failures: 2,
            global_failures: 3,
            maximum_keys: 8,
        });
        let now = Instant::now();
        throttle.record_failure_at("a", now);
        assert!(!throttle.blocked_at("a", now));
        throttle.record_failure_at("a", now);
        assert!(throttle.blocked_at("a", now));
        assert!(!throttle.blocked_at("b", now));
        throttle.record_failure_at("b", now);
        assert!(throttle.blocked_at("c", now));

        throttle.record_success("a");
        assert!(!throttle.blocked_at("a", now));
        assert!(!throttle.blocked_at("c", now));
        assert!(!throttle.blocked_at("a", now + Duration::from_secs(61)));
    }

    #[test]
    fn secret_token_clone_stays_redacted() {
        let token = SecretSessionToken::new("sensitive-token");
        assert_eq!(token.clone().expose_secret(), "sensitive-token");
        assert!(!format!("{token:?}").contains("sensitive"));
    }
}
