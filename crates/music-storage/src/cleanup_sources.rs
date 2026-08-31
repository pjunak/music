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
            sqlx::query(
                "INSERT INTO cleanup_source_policies (source_id, enabled, updated_at) \
                 VALUES (?, ?, CURRENT_TIMESTAMP) \
                 ON CONFLICT(source_id) DO UPDATE SET \
                    enabled = excluded.enabled, updated_at = excluded.updated_at",
            )
            .bind(source_id)
            .bind(enabled)
            .execute(&self.pool)
            .await?;
            Ok(())
        })
    }
}

#[cfg(test)]
mod tests {
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
        Ok(())
    }
}
