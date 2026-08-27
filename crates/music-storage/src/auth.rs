use std::time::Duration;

use music_application::auth::{
    AuthFuture, AuthRepository, DependencyError, PasswordHash, PasswordVerifier,
    RevokeSessionOutcome, SecretSessionToken, SessionLookup, SessionTouch, StoredSessionSummary,
    UnixSeconds, UserCredentialRecord, UserInfo,
};
use sqlx::Row;

use crate::{SqliteStorage, StorageError, verify_password};

const DUMMY_PASSWORD_HASH: &str = "$argon2id$v=19$m=65536,t=3,p=4$cmV3cml0ZS1maXh0dXJlLXNhbHQ$tgdYDN8ijOk+7HZgF2oUm50+O8nOMKGggRmynxNu4ko";

#[derive(Debug, Default)]
pub struct Argon2PasswordVerifier;

impl PasswordVerifier for Argon2PasswordVerifier {
    fn verify_candidate(
        &self,
        password_hash: Option<&str>,
        candidate: &str,
    ) -> Result<bool, DependencyError> {
        let encoded = password_hash.unwrap_or(DUMMY_PASSWORD_HASH);
        let valid = verify_password(encoded, candidate)
            .map_err(|source| -> DependencyError { Box::new(source) })?;
        Ok(password_hash.is_some() && valid)
    }
}

impl SqliteStorage {
    pub async fn create_user(
        &self,
        username: &str,
        password_hash: &str,
        created_at: UnixSeconds,
    ) -> Result<i64, StorageError> {
        let _admission = self.write_gate.lock().await;
        let result = sqlx::query(
            "INSERT INTO users (username, password_hash, created_at) \
             VALUES (?, ?, datetime(?, 'unixepoch'))",
        )
        .bind(username)
        .bind(password_hash)
        .bind(created_at.get())
        .execute(&self.pool)
        .await?;
        Ok(result.last_insert_rowid())
    }

    pub async fn replace_password_hash(
        &self,
        user_id: i64,
        password_hash: &str,
    ) -> Result<bool, StorageError> {
        let _admission = self.write_gate.lock().await;
        let result = sqlx::query("UPDATE users SET password_hash = ? WHERE id = ?")
            .bind(password_hash)
            .bind(user_id)
            .execute(&self.pool)
            .await?;
        Ok(result.rows_affected() == 1)
    }

    /// Replace one user's password and optionally revoke every active session
    /// in the same transaction. Offline administration must never expose a
    /// window where a new password is committed while leaked cookies survive.
    pub async fn replace_user_password(
        &self,
        username: &str,
        password_hash: &str,
        revoke_sessions: bool,
    ) -> Result<Option<u64>, StorageError> {
        let _admission = self.write_gate.lock().await;
        let mut transaction = self.pool.begin().await?;
        let user_id = sqlx::query_scalar::<_, i64>("SELECT id FROM users WHERE username = ?")
            .bind(username)
            .fetch_optional(&mut *transaction)
            .await?;
        let Some(user_id) = user_id else {
            transaction.rollback().await?;
            return Ok(None);
        };
        sqlx::query("UPDATE users SET password_hash = ? WHERE id = ?")
            .bind(password_hash)
            .bind(user_id)
            .execute(&mut *transaction)
            .await?;
        let revoked = if revoke_sessions {
            sqlx::query("DELETE FROM auth_sessions WHERE user_id = ?")
                .bind(user_id)
                .execute(&mut *transaction)
                .await?
                .rows_affected()
        } else {
            0
        };
        transaction.commit().await?;
        Ok(Some(revoked))
    }
}

impl AuthRepository for SqliteStorage {
    fn find_user_by_username<'a>(
        &'a self,
        username: &'a str,
    ) -> AuthFuture<'a, Option<UserCredentialRecord>> {
        Box::pin(async move {
            let row =
                sqlx::query("SELECT id, username, password_hash FROM users WHERE username = ?")
                    .bind(username)
                    .fetch_optional(&self.pool)
                    .await
                    .map_err(box_storage)?;
            row.map(|row| {
                Ok(UserCredentialRecord {
                    user: UserInfo {
                        id: row.try_get("id").map_err(box_sqlx)?,
                        username: row.try_get("username").map_err(box_sqlx)?,
                    },
                    password_hash: PasswordHash::new(
                        row.try_get::<String, _>("password_hash")
                            .map_err(box_sqlx)?,
                    ),
                })
            })
            .transpose()
        })
    }

    fn create_session<'a>(
        &'a self,
        user_id: i64,
        token: &'a str,
        created_at: UnixSeconds,
        expires_at: UnixSeconds,
    ) -> AuthFuture<'a, ()> {
        Box::pin(async move {
            let _admission = self.write_gate.lock().await;
            sqlx::query(
                "INSERT INTO auth_sessions \
                 (token, user_id, created_at, expires_at, last_seen) \
                 VALUES (?, ?, datetime(?, 'unixepoch'), datetime(?, 'unixepoch'), \
                         datetime(?, 'unixepoch'))",
            )
            .bind(token)
            .bind(user_id)
            .bind(created_at.get())
            .bind(expires_at.get())
            .bind(created_at.get())
            .execute(&self.pool)
            .await
            .map_err(box_storage)?;
            Ok(())
        })
    }

    fn lookup_session<'a>(
        &'a self,
        token: &'a str,
        now: UnixSeconds,
        touch: SessionTouch,
        last_seen_throttle: Duration,
    ) -> AuthFuture<'a, SessionLookup> {
        Box::pin(async move {
            let row = sqlx::query(
                "SELECT s.user_id, u.username, unixepoch(s.expires_at) AS expires_at_epoch, \
                        unixepoch(s.last_seen) AS last_seen_epoch \
                 FROM auth_sessions AS s \
                 LEFT JOIN users AS u ON u.id = s.user_id \
                 WHERE s.token = ?",
            )
            .bind(token)
            .fetch_optional(&self.pool)
            .await
            .map_err(box_storage)?;
            let Some(row) = row else {
                return Ok(SessionLookup::Missing);
            };
            let expires_at = required_epoch(&row, "expires_at_epoch")?;
            if expires_at <= now {
                let _admission = self.write_gate.lock().await;
                sqlx::query("DELETE FROM auth_sessions WHERE token = ?")
                    .bind(token)
                    .execute(&self.pool)
                    .await
                    .map_err(box_storage)?;
                return Ok(SessionLookup::Expired);
            }
            let Some(username) = row
                .try_get::<Option<String>, _>("username")
                .map_err(box_sqlx)?
            else {
                return Ok(SessionLookup::Missing);
            };
            if touch == SessionTouch::UpdateLastSeen {
                let last_seen = required_epoch(&row, "last_seen_epoch")?;
                let throttle_seconds =
                    i64::try_from(last_seen_throttle.as_secs()).unwrap_or(i64::MAX);
                if now.get().saturating_sub(last_seen.get()) > throttle_seconds {
                    let _admission = self.write_gate.lock().await;
                    let result = sqlx::query(
                        "UPDATE auth_sessions SET last_seen = datetime(?, 'unixepoch') \
                         WHERE token = ? AND unixepoch(expires_at) > ?",
                    )
                    .bind(now.get())
                    .bind(token)
                    .bind(now.get())
                    .execute(&self.pool)
                    .await
                    .map_err(box_storage)?;
                    if result.rows_affected() == 0 {
                        return Ok(SessionLookup::Missing);
                    }
                }
            }
            Ok(SessionLookup::Authenticated {
                user: UserInfo {
                    id: row.try_get("user_id").map_err(box_sqlx)?,
                    username,
                },
                expires_at,
            })
        })
    }

    fn delete_sessions_for_user(&self, user_id: i64) -> AuthFuture<'_, u64> {
        Box::pin(async move {
            let _admission = self.write_gate.lock().await;
            sqlx::query("DELETE FROM auth_sessions WHERE user_id = ?")
                .bind(user_id)
                .execute(&self.pool)
                .await
                .map(|result| result.rows_affected())
                .map_err(box_storage)
        })
    }

    fn list_sessions(&self, user_id: i64) -> AuthFuture<'_, Vec<StoredSessionSummary>> {
        Box::pin(async move {
            let rows = sqlx::query(
                "SELECT token, unixepoch(created_at) AS created_at_epoch, \
                        unixepoch(expires_at) AS expires_at_epoch, \
                        unixepoch(last_seen) AS last_seen_epoch \
                 FROM auth_sessions WHERE user_id = ? ORDER BY last_seen DESC",
            )
            .bind(user_id)
            .fetch_all(&self.pool)
            .await
            .map_err(box_storage)?;
            rows.iter()
                .map(|row| {
                    Ok(StoredSessionSummary {
                        token: SecretSessionToken::new(
                            row.try_get::<String, _>("token").map_err(box_sqlx)?,
                        ),
                        created_at: required_epoch(row, "created_at_epoch")?,
                        expires_at: required_epoch(row, "expires_at_epoch")?,
                        last_seen: required_epoch(row, "last_seen_epoch")?,
                    })
                })
                .collect()
        })
    }

    fn revoke_session_prefix<'a>(
        &'a self,
        user_id: i64,
        token_prefix: &'a str,
    ) -> AuthFuture<'a, RevokeSessionOutcome> {
        Box::pin(async move {
            let _admission = self.write_gate.lock().await;
            let matches = sqlx::query_scalar::<_, String>(
                "SELECT token FROM auth_sessions \
                 WHERE user_id = ? AND substr(token, 1, length(?)) = ? LIMIT 2",
            )
            .bind(user_id)
            .bind(token_prefix)
            .bind(token_prefix)
            .fetch_all(&self.pool)
            .await
            .map_err(box_storage)?;
            match matches.as_slice() {
                [] => Ok(RevokeSessionOutcome::Missing),
                [token] => {
                    sqlx::query("DELETE FROM auth_sessions WHERE token = ?")
                        .bind(token)
                        .execute(&self.pool)
                        .await
                        .map_err(box_storage)?;
                    Ok(RevokeSessionOutcome::Revoked)
                }
                _ => Ok(RevokeSessionOutcome::Ambiguous),
            }
        })
    }
}

fn required_epoch(
    row: &sqlx::sqlite::SqliteRow,
    column: &str,
) -> Result<UnixSeconds, DependencyError> {
    row.try_get::<Option<i64>, _>(column)
        .map_err(box_sqlx)?
        .map(UnixSeconds::new)
        .ok_or_else(|| Box::new(StorageError::InvalidTimestamp) as DependencyError)
}

fn box_storage(source: sqlx::Error) -> DependencyError {
    Box::new(StorageError::Database(source))
}

fn box_sqlx(source: sqlx::Error) -> DependencyError {
    box_storage(source)
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;
    use std::time::Duration;

    use music_application::auth::{
        AuthClock, AuthRepository, AuthService, AuthServiceConfig, PasswordVerifier,
        RevokeSessionOutcome, SecretSessionToken, SessionLookup, SessionTokenSource, SessionTouch,
        UnixSeconds,
    };
    use tempfile::tempdir;

    use super::Argon2PasswordVerifier;
    use crate::{SqliteStorage, SqliteStorageOptions, hash_password};

    #[derive(Debug)]
    struct FixedToken;

    impl SessionTokenSource for FixedToken {
        fn generate(&self) -> Result<SecretSessionToken, music_application::auth::DependencyError> {
            Ok(SecretSessionToken::new(
                "abcdefghijklmnopqrstuvwxabcdefghijklmnopqrstuvwxabcdefghijklmnop",
            ))
        }
    }

    #[derive(Debug)]
    struct FixedClock;

    impl AuthClock for FixedClock {
        fn now(&self) -> Result<UnixSeconds, music_application::auth::DependencyError> {
            Ok(UnixSeconds::new(1_800_000_000))
        }
    }

    async fn test_storage()
    -> Result<(tempfile::TempDir, Arc<SqliteStorage>), Box<dyn std::error::Error + Send + Sync>>
    {
        let directory = tempdir()?;
        let storage = Arc::new(
            SqliteStorage::open(SqliteStorageOptions::new(directory.path().join("app.db"))).await?,
        );
        Ok((directory, storage))
    }

    #[test]
    fn verifier_accepts_python_hash_and_equalizes_unknown_users()
    -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
        let verifier = Argon2PasswordVerifier;
        let hash = super::DUMMY_PASSWORD_HASH;
        assert!(verifier.verify_candidate(Some(hash), "rewrite-fixture-password")?);
        assert!(!verifier.verify_candidate(Some(hash), "wrong")?);
        assert!(!verifier.verify_candidate(None, "rewrite-fixture-password")?);
        Ok(())
    }

    #[tokio::test]
    async fn login_resolve_list_revoke_and_expire_are_one_repository_contract()
    -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
        let (_directory, storage) = test_storage().await?;
        let hash = hash_password("correct horse battery staple")?;
        let user_id = storage
            .create_user("operator", &hash, UnixSeconds::new(1_799_000_000))
            .await?;
        let service = AuthService::new(
            Arc::clone(&storage),
            Arc::new(Argon2PasswordVerifier),
            Arc::new(FixedToken),
            Arc::new(FixedClock),
            AuthServiceConfig::new(30)?,
        );

        let login = service
            .login("operator", "correct horse battery staple")
            .await?;
        assert_eq!(login.user.id, user_id);
        assert_eq!(login.token.expose_secret().len(), 64);
        assert!(matches!(
            service
                .authenticate(login.token.expose_secret(), SessionTouch::UpdateLastSeen)
                .await?,
            SessionLookup::Authenticated { user, .. } if user.username == "operator"
        ));
        let sessions = service
            .list_sessions(user_id, login.token.expose_secret())
            .await?;
        assert_eq!(sessions.len(), 1);
        assert!(sessions[0].is_current);
        assert_eq!(sessions[0].token_prefix, "abcdefghijkl");
        assert_eq!(
            service.revoke_session(user_id, "abcdefgh").await?,
            RevokeSessionOutcome::Revoked
        );
        assert_eq!(
            service
                .authenticate(login.token.expose_secret(), SessionTouch::PreserveLastSeen)
                .await?,
            SessionLookup::Missing
        );

        AuthRepository::create_session(
            storage.as_ref(),
            user_id,
            "expired-token",
            UnixSeconds::new(1_700_000_000),
            UnixSeconds::new(1_700_000_001),
        )
        .await?;
        assert_eq!(
            AuthRepository::lookup_session(
                storage.as_ref(),
                "expired-token",
                UnixSeconds::new(1_800_000_000),
                SessionTouch::PreserveLastSeen,
                Duration::from_secs(60),
            )
            .await?,
            SessionLookup::Expired
        );
        assert_eq!(
            AuthRepository::lookup_session(
                storage.as_ref(),
                "expired-token",
                UnixSeconds::new(1_800_000_000),
                SessionTouch::PreserveLastSeen,
                Duration::from_secs(60),
            )
            .await?,
            SessionLookup::Missing
        );
        Ok(())
    }

    #[tokio::test]
    async fn offline_password_replacement_is_atomic_with_session_revocation()
    -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
        let (_directory, storage) = test_storage().await?;
        let old_hash = hash_password("old operator password")?;
        let new_hash = hash_password("new operator password")?;
        let user_id = storage
            .create_user("operator", &old_hash, UnixSeconds::new(1_799_000_000))
            .await?;
        for token in ["first-active-token", "second-active-token"] {
            AuthRepository::create_session(
                storage.as_ref(),
                user_id,
                token,
                UnixSeconds::new(1_799_999_000),
                UnixSeconds::new(1_800_001_000),
            )
            .await?;
        }

        assert_eq!(
            storage
                .replace_user_password("operator", &new_hash, true)
                .await?,
            Some(2)
        );
        let record = AuthRepository::find_user_by_username(storage.as_ref(), "operator")
            .await?
            .ok_or("operator disappeared after password replacement")?;
        assert_eq!(record.password_hash.as_str(), new_hash);
        for token in ["first-active-token", "second-active-token"] {
            assert_eq!(
                AuthRepository::lookup_session(
                    storage.as_ref(),
                    token,
                    UnixSeconds::new(1_800_000_000),
                    SessionTouch::PreserveLastSeen,
                    Duration::from_secs(60),
                )
                .await?,
                SessionLookup::Missing
            );
        }

        AuthRepository::create_session(
            storage.as_ref(),
            user_id,
            "preserved-token",
            UnixSeconds::new(1_799_999_000),
            UnixSeconds::new(1_800_001_000),
        )
        .await?;
        assert_eq!(
            storage
                .replace_user_password("operator", &old_hash, false)
                .await?,
            Some(0)
        );
        assert!(matches!(
            AuthRepository::lookup_session(
                storage.as_ref(),
                "preserved-token",
                UnixSeconds::new(1_800_000_000),
                SessionTouch::PreserveLastSeen,
                Duration::from_secs(60),
            )
            .await?,
            SessionLookup::Authenticated { .. }
        ));
        assert_eq!(
            storage
                .replace_user_password("missing", &new_hash, true)
                .await?,
            None
        );
        Ok(())
    }
}
