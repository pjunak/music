use music_application::assistant::EncryptedProviderCredential;
use music_application::cleanup_sources::{CleanupSourceFuture, CleanupSourceRepository};
use sqlx::Row;

use crate::SqliteStorage;

impl CleanupSourceRepository for SqliteStorage {
    fn cleanup_source_enabled(&self, source_id: &str) -> CleanupSourceFuture<'_, Option<bool>> {
        let source_id = source_id.to_owned();
        Box::pin(async move {
            let row =
                sqlx::query("SELECT enabled FROM cleanup_source_policies WHERE source_id = ?")
                    .bind(source_id)
                    .fetch_optional(&self.pool)
                    .await?;
            row.map(|row| row.try_get::<bool, _>("enabled"))
                .transpose()
                .map_err(Into::into)
        })
    }

    fn set_cleanup_source_enabled(
        &self,
        source_id: &str,
        enabled: bool,
    ) -> CleanupSourceFuture<'_, ()> {
        let source_id = source_id.to_owned();
        Box::pin(async move {
            let _admission = self.write_gate.lock().await;
            let mut transaction = self.pool.begin().await?;
            sqlx::query(
                "INSERT INTO cleanup_source_policies (source_id, enabled, updated_at) \
                 VALUES (?, ?, CURRENT_TIMESTAMP) \
                 ON CONFLICT(source_id) DO UPDATE SET \
                    enabled = excluded.enabled, updated_at = excluded.updated_at",
            )
            .bind(&source_id)
            .bind(enabled)
            .execute(&mut *transaction)
            .await?;
            crate::catalog_evidence::invalidate(&mut transaction).await?;
            transaction.commit().await?;
            Ok(())
        })
    }

    fn cleanup_source_credential(
        &self,
        source_id: &str,
    ) -> CleanupSourceFuture<'_, Option<EncryptedProviderCredential>> {
        let source_id = source_id.to_owned();
        Box::pin(async move {
            let row = sqlx::query(
                "SELECT encrypted_api_key, api_key_nonce, api_key_hint \
                 FROM cleanup_source_credentials WHERE source_id = ?",
            )
            .bind(source_id)
            .fetch_optional(&self.pool)
            .await?;
            match row {
                Some(row) => Ok(Some(EncryptedProviderCredential {
                    ciphertext: row.try_get("encrypted_api_key")?,
                    nonce: row.try_get("api_key_nonce")?,
                    hint: row.try_get("api_key_hint")?,
                })),
                None => Ok(None),
            }
        })
    }

    fn store_cleanup_source_credential<'a>(
        &'a self,
        source_id: &'a str,
        credential: &'a EncryptedProviderCredential,
    ) -> CleanupSourceFuture<'a, ()> {
        Box::pin(async move {
            let _admission = self.write_gate.lock().await;
            let mut transaction = self.pool.begin().await?;
            sqlx::query(
                "INSERT INTO cleanup_source_credentials \
                 (source_id, encrypted_api_key, api_key_nonce, api_key_hint) \
                 VALUES (?, ?, ?, ?) \
                 ON CONFLICT(source_id) DO UPDATE SET \
                    encrypted_api_key = excluded.encrypted_api_key, \
                    api_key_nonce = excluded.api_key_nonce, \
                    api_key_hint = excluded.api_key_hint, updated_at = CURRENT_TIMESTAMP",
            )
            .bind(source_id)
            .bind(&credential.ciphertext)
            .bind(&credential.nonce)
            .bind(&credential.hint)
            .execute(&mut *transaction)
            .await?;
            crate::catalog_evidence::invalidate(&mut transaction).await?;
            transaction.commit().await?;
            Ok(())
        })
    }

    fn clear_cleanup_source_credential(&self, source_id: &str) -> CleanupSourceFuture<'_, bool> {
        let source_id = source_id.to_owned();
        Box::pin(async move {
            let _admission = self.write_gate.lock().await;
            let mut transaction = self.pool.begin().await?;
            let deleted = sqlx::query("DELETE FROM cleanup_source_credentials WHERE source_id = ?")
                .bind(&source_id)
                .execute(&mut *transaction)
                .await?
                .rows_affected()
                > 0;
            if deleted {
                crate::catalog_evidence::invalidate(&mut transaction).await?;
            }
            transaction.commit().await?;
            Ok(deleted)
        })
    }
}

#[cfg(test)]
mod tests {
    use music_application::assistant::{EncryptedProviderCredential, ProviderRepository};
    use music_application::cleanup_sources::CleanupSourceRepository;
    use tempfile::tempdir;

    use crate::{SqliteStorage, SqliteStorageOptions};

    #[tokio::test]
    async fn cleanup_source_policy_round_trips()
    -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
        let directory = tempdir()?;
        let storage =
            SqliteStorage::open(SqliteStorageOptions::new(directory.path().join("app.db"))).await?;
        assert_eq!(storage.cleanup_source_enabled("musicbrainz").await?, None);

        storage
            .set_cleanup_source_enabled("musicbrainz", false)
            .await?;
        assert_eq!(
            storage.cleanup_source_enabled("musicbrainz").await?,
            Some(false)
        );

        let credential = EncryptedProviderCredential {
            ciphertext: "ciphertext".to_owned(),
            nonce: "nonce".to_owned(),
            hint: "••••hint".to_owned(),
        };
        storage
            .store_cleanup_source_credential("lastfm", &credential)
            .await?;
        assert!(storage.saved_provider_credentials_exist().await?);
        assert_eq!(
            storage.cleanup_source_credential("lastfm").await?,
            Some(credential)
        );
        assert!(storage.clear_cleanup_source_credential("lastfm").await?);
        assert_eq!(storage.cleanup_source_credential("lastfm").await?, None);
        assert!(!storage.saved_provider_credentials_exist().await?);
        Ok(())
    }
}
