use music_application::assistant::{
    AnalysisState, AnalysisWrite, AssistantFuture, LOCAL_METADATA_ANALYZER_ID,
    LocalAnalysisRepository, metadata_source_signature,
};
use sqlx::{AssertSqlSafe, Row};

use crate::library::{TRACK_COLUMNS, indexed_track_from_row};
use crate::{SqliteStorage, StorageError};

impl LocalAnalysisRepository for SqliteStorage {
    fn analysis_states<'a>(
        &'a self,
        analyzer_id: &'a str,
    ) -> AssistantFuture<'a, Vec<AnalysisState>> {
        Box::pin(async move {
            let rows = sqlx::query(
                "SELECT track_id, source_signature, job_id, confidence, \
                 CAST(strftime('%s', updated_at) AS INTEGER) AS updated_at_unix_seconds \
                 FROM track_analyses WHERE analyzer_id = ? ORDER BY track_id",
            )
            .bind(analyzer_id)
            .fetch_all(&self.pool)
            .await
            .map_err(box_storage)?;
            rows.iter()
                .map(|row| {
                    let raw_id: i64 = row.try_get("track_id").map_err(StorageError::from)?;
                    Ok(AnalysisState {
                        track_id: music_domain::TrackId::new(raw_id).map_err(|_| {
                            StorageError::InvalidAssistantRecord("analysis track id is invalid")
                        })?,
                        source_signature: row
                            .try_get("source_signature")
                            .map_err(StorageError::from)?,
                        job_id: row.try_get("job_id").map_err(StorageError::from)?,
                        confidence: row.try_get("confidence").map_err(StorageError::from)?,
                        updated_at_unix_seconds: row
                            .try_get::<Option<i64>, _>("updated_at_unix_seconds")
                            .map_err(StorageError::from)?
                            .ok_or(StorageError::InvalidAssistantRecord(
                                "analysis timestamp is invalid",
                            ))?,
                    })
                })
                .collect::<Result<Vec<_>, StorageError>>()
                .map_err(box_storage)
        })
    }

    fn store_metadata_analysis<'a>(
        &'a self,
        analyzer_id: &'a str,
        job_id: &'a str,
        profiles: &'a [AnalysisWrite],
    ) -> AssistantFuture<'a, usize> {
        Box::pin(async move {
            if analyzer_id != LOCAL_METADATA_ANALYZER_ID {
                return Err(box_storage(StorageError::InvalidAssistantRecord(
                    "metadata analyzer id is invalid",
                )));
            }
            let _admission = self.write_gate.lock().await;
            let mut transaction = self.pool.begin().await.map_err(box_storage)?;
            let mut stored = 0_usize;
            for profile in profiles {
                if !valid_profile(profile) {
                    return Err(box_storage(StorageError::InvalidAssistantRecord(
                        "metadata analysis profile is invalid",
                    )));
                }
                let query = format!("SELECT {TRACK_COLUMNS} FROM tracks WHERE id = ?");
                let row = sqlx::query(AssertSqlSafe(query))
                    .bind(profile.track_id.get())
                    .fetch_optional(&mut *transaction)
                    .await
                    .map_err(box_storage)?;
                let Some(row) = row else {
                    continue;
                };
                let track = indexed_track_from_row(&row).map_err(box_storage)?;
                let current_signature = metadata_source_signature(&track).map_err(|_| {
                    box_storage(StorageError::InvalidAssistantRecord(
                        "track metadata fingerprint is invalid",
                    ))
                })?;
                if current_signature != profile.source_signature {
                    continue;
                }
                let moods = serde_json::to_string(&profile.moods)
                    .map_err(StorageError::AssistantSerialization)
                    .map_err(box_storage)?;
                let evidence = serde_json::to_string(&profile.evidence)
                    .map_err(StorageError::AssistantSerialization)
                    .map_err(box_storage)?;
                sqlx::query(
                    "INSERT INTO track_analyses \
                     (track_id, analyzer_id, source_signature, job_id, energy, brightness, \
                      tension, moods_json, evidence_json, metrics_json, confidence, updated_at) \
                     VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, '{}', ?, CURRENT_TIMESTAMP) \
                     ON CONFLICT(track_id, analyzer_id) DO UPDATE SET \
                       source_signature = excluded.source_signature, job_id = excluded.job_id, \
                       energy = excluded.energy, brightness = excluded.brightness, \
                       tension = excluded.tension, moods_json = excluded.moods_json, \
                       evidence_json = excluded.evidence_json, confidence = excluded.confidence, \
                       updated_at = CURRENT_TIMESTAMP",
                )
                .bind(profile.track_id.get())
                .bind(analyzer_id)
                .bind(&profile.source_signature)
                .bind(job_id)
                .bind(profile.energy)
                .bind(profile.brightness)
                .bind(profile.tension)
                .bind(moods)
                .bind(evidence)
                .bind(profile.confidence.as_str())
                .execute(&mut *transaction)
                .await
                .map_err(box_storage)?;
                stored = stored.saturating_add(1);
            }
            transaction.commit().await.map_err(box_storage)?;
            Ok(stored)
        })
    }
}

fn valid_profile(profile: &AnalysisWrite) -> bool {
    [profile.energy, profile.brightness, profile.tension]
        .into_iter()
        .all(|value| value.is_finite() && (0.0..=1.0).contains(&value))
        && profile.source_signature.len() == 64
        && !profile.evidence.is_empty()
}

fn box_storage(error: impl Into<StorageError>) -> Box<dyn std::error::Error + Send + Sync> {
    Box::new(error.into())
}

#[cfg(test)]
mod tests {
    use std::error::Error;

    use music_application::assistant::{
        AnalysisWrite, Confidence, LOCAL_METADATA_ANALYZER_ID, LocalAnalysisRepository,
        metadata_source_signature,
    };
    use music_application::library::LibraryRepository;
    use tempfile::TempDir;

    use crate::{SqliteStorage, SqliteStorageOptions};

    async fn storage() -> Result<(TempDir, SqliteStorage), Box<dyn Error + Send + Sync>> {
        let directory = tempfile::tempdir()?;
        let storage =
            SqliteStorage::open(SqliteStorageOptions::new(directory.path().join("music.db")))
                .await?;
        sqlx::query("INSERT INTO tracks (path, title, artist, album_artist, album, track_no, disc_no, year, genre, length_s, bpm, display_title, origin, size_bytes, mtime, added_at) VALUES ('battle.flac', 'Battle', 'Composer', '', '', NULL, NULL, NULL, 'Cinematic', 60.0, 160, '', '', 10, 20, CURRENT_TIMESTAMP)")
            .execute(&storage.pool)
            .await?;
        Ok((directory, storage))
    }

    fn write(
        track: &music_domain::IndexedTrack,
    ) -> Result<AnalysisWrite, Box<dyn Error + Send + Sync>> {
        Ok(AnalysisWrite {
            track_id: track.id,
            source_signature: metadata_source_signature(track)?,
            energy: 0.8,
            brightness: 0.5,
            tension: 0.7,
            moods: vec!["combat".to_owned()],
            evidence: vec!["Mood metadata: combat".to_owned()],
            confidence: Confidence::High,
        })
    }

    #[tokio::test]
    async fn metadata_analysis_rechecks_source_identity_inside_the_write_transaction()
    -> Result<(), Box<dyn Error + Send + Sync>> {
        let (_directory, storage) = storage().await?;
        let original = LibraryRepository::all_tracks(&storage).await?.remove(0);
        let stale = write(&original)?;
        sqlx::query("UPDATE tracks SET title = 'Changed' WHERE id = ?")
            .bind(original.id.get())
            .execute(&storage.pool)
            .await?;
        assert_eq!(
            LocalAnalysisRepository::store_metadata_analysis(
                &storage,
                LOCAL_METADATA_ANALYZER_ID,
                "job-a",
                &[stale],
            )
            .await?,
            0
        );
        let current = LibraryRepository::all_tracks(&storage).await?.remove(0);
        assert_eq!(
            LocalAnalysisRepository::store_metadata_analysis(
                &storage,
                LOCAL_METADATA_ANALYZER_ID,
                "job-b",
                &[write(&current)?],
            )
            .await?,
            1
        );
        let states =
            LocalAnalysisRepository::analysis_states(&storage, LOCAL_METADATA_ANALYZER_ID).await?;
        assert_eq!(states.len(), 1);
        assert_eq!(states[0].job_id, "job-b");
        Ok(())
    }
}
