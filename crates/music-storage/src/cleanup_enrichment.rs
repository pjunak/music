use music_application::cleanup_enrichment::{
    CleanupEnrichmentDependencyError, CleanupEnrichmentFuture, CleanupEnrichmentRecord,
    CleanupEnrichmentRepository, cleanup_enrichment_source_signature,
};
use music_domain::TrackId;
use sqlx::{AssertSqlSafe, Row};

use crate::library::{TRACK_COLUMNS, indexed_track_from_row};
use crate::{SqliteStorage, StorageError};

impl CleanupEnrichmentRepository for SqliteStorage {
    fn cleanup_enrichment(
        &self,
        track_id: TrackId,
    ) -> CleanupEnrichmentFuture<'_, Option<CleanupEnrichmentRecord>> {
        Box::pin(async move {
            let row = sqlx::query(
                "SELECT source_signature, result_json FROM cleanup_track_enrichments WHERE track_id = ?",
            )
            .bind(track_id.get())
            .fetch_optional(&self.pool)
            .await?;
            row.map(|row| {
                let result = serde_json::from_str::<serde_json::Value>(
                    row.try_get::<&str, _>("result_json")?,
                )
                .map_err(StorageError::AssistantSerialization)?;
                let result =
                    result
                        .as_object()
                        .cloned()
                        .ok_or(StorageError::InvalidAssistantRecord(
                            "cleanup enrichment result is invalid",
                        ))?;
                Ok::<_, StorageError>(CleanupEnrichmentRecord {
                    track_id,
                    source_signature: row.try_get("source_signature")?,
                    result,
                })
            })
            .transpose()
            .map_err(box_storage)
        })
    }

    fn store_cleanup_enrichment<'a>(
        &'a self,
        record: &'a CleanupEnrichmentRecord,
    ) -> CleanupEnrichmentFuture<'a, bool> {
        Box::pin(async move {
            if record.source_signature.len() != 64 || record.result.is_empty() {
                return Err(box_storage(StorageError::InvalidAssistantRecord(
                    "cleanup enrichment record is invalid",
                )));
            }
            let result = serde_json::to_string(&record.result)
                .map_err(StorageError::AssistantSerialization)
                .map_err(box_storage)?;
            let _admission = self.write_gate.lock().await;
            let query = format!("SELECT {TRACK_COLUMNS} FROM tracks WHERE id = ?");
            let row = sqlx::query(AssertSqlSafe(query))
                .bind(record.track_id.get())
                .fetch_optional(&self.pool)
                .await
                .map_err(box_storage)?;
            let Some(row) = row else {
                return Ok(false);
            };
            let track = indexed_track_from_row(&row).map_err(box_storage)?;
            let current_signature = cleanup_enrichment_source_signature(&track)
                .map_err(|_| {
                    StorageError::InvalidAssistantRecord("cleanup enrichment signature is invalid")
                })
                .map_err(box_storage)?;
            if current_signature != record.source_signature {
                return Ok(false);
            }
            sqlx::query(
                "INSERT INTO cleanup_track_enrichments \
                 (track_id, source_signature, result_json, updated_at) \
                 VALUES (?, ?, ?, CURRENT_TIMESTAMP) \
                 ON CONFLICT(track_id) DO UPDATE SET \
                    source_signature = excluded.source_signature, \
                    result_json = excluded.result_json, updated_at = CURRENT_TIMESTAMP",
            )
            .bind(record.track_id.get())
            .bind(&record.source_signature)
            .bind(result)
            .execute(&self.pool)
            .await
            .map_err(box_storage)?;
            Ok(true)
        })
    }
}

fn box_storage(source: impl Into<StorageError>) -> CleanupEnrichmentDependencyError {
    Box::new(source.into())
}

#[cfg(test)]
mod tests {
    use std::error::Error;

    use music_application::cleanup_enrichment::{
        CleanupEnrichmentRecord, CleanupEnrichmentRepository, cleanup_enrichment_source_signature,
    };
    use music_application::library::LibraryRepository;
    use music_domain::TrackId;
    use tempfile::tempdir;

    use crate::{SqliteStorage, SqliteStorageOptions};

    #[tokio::test]
    async fn enrichment_cache_is_bound_to_current_track_metadata()
    -> Result<(), Box<dyn Error + Send + Sync>> {
        let directory = tempdir()?;
        let storage =
            SqliteStorage::open(SqliteStorageOptions::new(directory.path().join("app.db"))).await?;
        sqlx::query(
            "INSERT INTO tracks \
             (path, title, artist, album_artist, album, track_no, disc_no, year, genre, length_s, \
              bpm, size_bytes, mtime, added_at, display_title, origin) \
             VALUES ('album/song.mp3', 'Song', 'Artist', '', 'Album', 1, 1, 2026, '', 120.0, \
                     NULL, 10, 20, CURRENT_TIMESTAMP, '', '')",
        )
        .execute(&storage.pool)
        .await?;
        let track_id = TrackId::new(1)?;
        let track = storage.track(track_id).await?.ok_or("missing track")?;
        let record = CleanupEnrichmentRecord {
            track_id,
            source_signature: cleanup_enrichment_source_signature(&track)?,
            result: serde_json::json!({"schema":"library-cleanup-enrichment/v1"})
                .as_object()
                .cloned()
                .ok_or("invalid fixture")?,
        };
        assert!(storage.store_cleanup_enrichment(&record).await?);
        assert_eq!(storage.cleanup_enrichment(track_id).await?, Some(record));
        Ok(())
    }
}
