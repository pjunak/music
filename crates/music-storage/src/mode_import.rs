use std::collections::BTreeSet;
use std::error::Error;

use music_application::modes::{
    ModeMutationDataEffects, ModeMutationError, ModeMutationFailureKind, ModeMutationFuture,
};
use music_application::recovery::{
    RecoveryDomain, RecoveryJournalEntry, RecoveryState, validate_recovery_progress,
};
use serde::Deserialize;
use serde_json::Value;

use crate::{SqliteStorage, StorageError};

const IMPORT_OPERATION: &str = "import_mode_resources";
const PLAYLIST_PROGRESS_KEY: &str = "authoring_playlist_ids";
const MAX_IMPORT_PLAYLISTS: usize = 256;
const MAX_IMPORT_PLAYLIST_ITEMS: usize = 10_000;

#[derive(Debug, Deserialize)]
struct ImportPlan {
    kind: ImportPlanKind,
    target: String,
    #[serde(default)]
    playlists: Vec<ImportPlaylist>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "snake_case")]
enum ImportPlanKind {
    ImportResources,
}

#[derive(Debug, Deserialize)]
struct ImportPlaylist {
    name: String,
    category: Option<String>,
    track_ids: Vec<i64>,
}

impl ModeMutationDataEffects for SqliteStorage {
    fn apply<'a>(
        &'a self,
        journal: &'a RecoveryJournalEntry,
    ) -> ModeMutationFuture<'a, RecoveryJournalEntry> {
        Box::pin(async move {
            if journal.operation.as_str() != IMPORT_OPERATION {
                return Ok(journal.clone());
            }
            apply_import(self, journal).await.map_err(box_import)
        })
    }

    fn rollback<'a>(&'a self, journal: &'a RecoveryJournalEntry) -> ModeMutationFuture<'a, ()> {
        Box::pin(async move {
            if journal.operation.as_str() != IMPORT_OPERATION {
                return Ok(());
            }
            rollback_import(self, journal).await.map_err(box_storage)
        })
    }

    fn finish<'a>(&'a self, _journal: &'a RecoveryJournalEntry) -> ModeMutationFuture<'a, ()> {
        Box::pin(async { Ok(()) })
    }

    fn cleanup_orphans(&self) -> ModeMutationFuture<'_, ()> {
        Box::pin(async { Ok(()) })
    }
}

async fn apply_import(
    storage: &SqliteStorage,
    journal: &RecoveryJournalEntry,
) -> Result<RecoveryJournalEntry, StorageError> {
    validate_import_journal(journal, RecoveryState::Applying)?;
    let plan = decode_plan(journal)?;
    if progress_playlist_ids(journal)?.is_some() {
        return Ok(journal.clone());
    }
    validate_plan(&plan)?;

    let _admission = storage.write_gate.lock().await;
    let mut transaction = storage.pool.begin().await?;
    let mut names = BTreeSet::new();
    let mut track_ids = BTreeSet::new();
    for playlist in &plan.playlists {
        if !names.insert(playlist.name.as_str()) {
            return Err(StorageError::InvalidLibraryState(
                "mode import contains duplicate playlist names",
            ));
        }
        let conflict = sqlx::query_scalar::<_, i64>(
            "SELECT 1 FROM playlists WHERE mode_id = ? AND name = ? LIMIT 1",
        )
        .bind(&plan.target)
        .bind(&playlist.name)
        .fetch_optional(&mut *transaction)
        .await?;
        if conflict.is_some() {
            return Err(StorageError::InvalidLibraryState(
                "mode import playlist already exists",
            ));
        }
        track_ids.extend(playlist.track_ids.iter().copied());
    }
    for track_id in track_ids {
        if track_id <= 0 {
            return Err(StorageError::InvalidLibraryState(
                "mode import track id is invalid",
            ));
        }
        let exists = sqlx::query_scalar::<_, i64>("SELECT 1 FROM tracks WHERE id = ? LIMIT 1")
            .bind(track_id)
            .fetch_optional(&mut *transaction)
            .await?;
        if exists.is_none() {
            return Err(StorageError::InvalidLibraryState(
                "mode import track does not exist",
            ));
        }
    }

    let mut created_ids = Vec::with_capacity(plan.playlists.len());
    for playlist in &plan.playlists {
        let result = sqlx::query(
            "INSERT INTO playlists (name, mode_id, category, automatic_rule_json, created_at, updated_at) \
             VALUES (?, ?, ?, '', CURRENT_TIMESTAMP, CURRENT_TIMESTAMP)",
        )
        .bind(&playlist.name)
        .bind(&plan.target)
        .bind(&playlist.category)
        .execute(&mut *transaction)
        .await?;
        let playlist_id = result.last_insert_rowid();
        for (position, track_id) in playlist.track_ids.iter().enumerate() {
            let position = i64::try_from(position).map_err(|_| {
                StorageError::InvalidLibraryState("mode import playlist position is invalid")
            })?;
            sqlx::query(
                "INSERT INTO playlist_items (playlist_id, position, track_id, added_at) \
                 VALUES (?, ?, ?, CURRENT_TIMESTAMP)",
            )
            .bind(playlist_id)
            .bind(position)
            .bind(track_id)
            .execute(&mut *transaction)
            .await?;
        }
        created_ids.push(playlist_id);
    }

    let mut progress =
        journal
            .progress
            .as_object()
            .cloned()
            .ok_or(StorageError::InvalidLibraryState(
                "mode import recovery progress is invalid",
            ))?;
    progress.insert(
        PLAYLIST_PROGRESS_KEY.to_owned(),
        serde_json::to_value(&created_ids).map_err(StorageError::RecoveryJournalSerialization)?,
    );
    progress.insert("stage".to_owned(), Value::String("data_applied".to_owned()));
    let progress = Value::Object(progress);
    validate_recovery_progress(&progress).map_err(StorageError::InvalidRecoveryJournal)?;
    let progress_json =
        serde_json::to_string(&progress).map_err(StorageError::RecoveryJournalSerialization)?;
    let updated = sqlx::query(
        "UPDATE recovery_journal SET progress_json = ?, updated_at = CURRENT_TIMESTAMP \
         WHERE id = ? AND state = 'applying'",
    )
    .bind(progress_json)
    .bind(journal.id.as_str())
    .execute(&mut *transaction)
    .await?;
    if updated.rows_affected() != 1 {
        return Err(StorageError::InvalidRecoveryTransition);
    }
    transaction.commit().await?;
    Ok(RecoveryJournalEntry {
        progress,
        ..journal.clone()
    })
}

async fn rollback_import(
    storage: &SqliteStorage,
    journal: &RecoveryJournalEntry,
) -> Result<(), StorageError> {
    if journal.domain != RecoveryDomain::Modes {
        return Err(StorageError::InvalidLibraryState(
            "mode import recovery domain is invalid",
        ));
    }
    let plan = decode_plan(journal)?;
    let Some(created_ids) = progress_playlist_ids(journal)? else {
        return Ok(());
    };
    if created_ids.len() != plan.playlists.len() {
        return Err(StorageError::InvalidLibraryState(
            "mode import recovery playlist count is invalid",
        ));
    }

    let _admission = storage.write_gate.lock().await;
    let mut transaction = storage.pool.begin().await?;
    for (playlist_id, playlist) in created_ids.iter().zip(&plan.playlists).rev() {
        let deleted =
            sqlx::query("DELETE FROM playlists WHERE id = ? AND mode_id = ? AND name = ?")
                .bind(playlist_id)
                .bind(&plan.target)
                .bind(&playlist.name)
                .execute(&mut *transaction)
                .await?;
        if deleted.rows_affected() == 0 {
            let exists =
                sqlx::query_scalar::<_, i64>("SELECT 1 FROM playlists WHERE id = ? LIMIT 1")
                    .bind(playlist_id)
                    .fetch_optional(&mut *transaction)
                    .await?;
            if exists.is_some() {
                return Err(StorageError::InvalidLibraryState(
                    "mode import playlist changed before rollback",
                ));
            }
        }
    }
    transaction.commit().await?;
    Ok(())
}

fn validate_import_journal(
    journal: &RecoveryJournalEntry,
    expected_state: RecoveryState,
) -> Result<(), StorageError> {
    if journal.domain != RecoveryDomain::Modes
        || journal.operation.as_str() != IMPORT_OPERATION
        || journal.state != expected_state
    {
        return Err(StorageError::InvalidLibraryState(
            "mode import recovery journal is invalid",
        ));
    }
    Ok(())
}

fn decode_plan(journal: &RecoveryJournalEntry) -> Result<ImportPlan, StorageError> {
    serde_json::from_value(journal.plan.clone()).map_err(StorageError::RecoveryJournalSerialization)
}

fn validate_plan(plan: &ImportPlan) -> Result<(), StorageError> {
    if !matches!(plan.kind, ImportPlanKind::ImportResources)
        || plan.playlists.len() > MAX_IMPORT_PLAYLISTS
        || plan.playlists.iter().any(|playlist| {
            playlist.name.is_empty() || playlist.track_ids.len() > MAX_IMPORT_PLAYLIST_ITEMS
        })
    {
        return Err(StorageError::InvalidLibraryState(
            "mode import data plan is invalid",
        ));
    }
    Ok(())
}

fn progress_playlist_ids(journal: &RecoveryJournalEntry) -> Result<Option<Vec<i64>>, StorageError> {
    let Some(value) = journal.progress.get(PLAYLIST_PROGRESS_KEY) else {
        return Ok(None);
    };
    serde_json::from_value(value.clone())
        .map(Some)
        .map_err(StorageError::RecoveryJournalSerialization)
}

fn box_storage(error: StorageError) -> Box<dyn Error + Send + Sync> {
    Box::new(error)
}

fn box_import(error: StorageError) -> Box<dyn Error + Send + Sync> {
    if matches!(
        &error,
        StorageError::InvalidLibraryState("mode import playlist already exists")
    ) {
        Box::new(ModeMutationError::new(
            ModeMutationFailureKind::Conflict,
            "target playlist appeared during import",
        ))
    } else {
        box_storage(error)
    }
}

#[cfg(test)]
mod tests {
    use std::error::Error;

    use music_application::modes::ModeMutationDataEffects;
    use music_application::recovery::{
        RecoveryDomain, RecoveryJournalDraft, RecoveryJournalId, RecoveryJournalRepository,
        RecoveryOperation, RecoveryState, RecoveryTransition,
    };
    use serde_json::json;
    use tempfile::tempdir;

    use crate::{SqliteStorage, SqliteStorageOptions};

    #[tokio::test]
    async fn playlist_import_is_transactional_idempotent_and_recoverable()
    -> Result<(), Box<dyn Error + Send + Sync>> {
        let directory = tempdir()?;
        let storage =
            SqliteStorage::open(SqliteStorageOptions::new(directory.path().join("app.db"))).await?;
        sqlx::query(
            "INSERT INTO tracks (id, path, title, artist, album_artist, album, genre, length_s, \
             display_title, origin, size_bytes, mtime, added_at) \
             VALUES (7, 'Album/track.wav', 'Track', '', '', '', '', 1.0, 'Track', '', 1, 1, CURRENT_TIMESTAMP)",
        )
        .execute(&storage.pool)
        .await?;

        let id = RecoveryJournalId::new();
        let plan = json!({
            "version": 1,
            "journal_id": id.as_str(),
            "kind": "import_resources",
            "target": "table",
            "stage_directory": format!(".music-mode-journal/{}", id.as_str()),
            "candidate": null,
            "backup": null,
            "target_existed": true,
            "candidate_sha256": null,
            "writes": [],
            "playlists": [{
                "name": "Night Walk",
                "category": "exploration",
                "track_ids": [7, 7]
            }]
        });
        let mut draft = RecoveryJournalDraft::new(
            RecoveryDomain::Modes,
            RecoveryOperation::parse("import_mode_resources")?,
            plan,
        )?;
        draft.id = id;
        let planned = storage.create_recovery_journal(draft).await?;
        let applying = applied(
            storage
                .transition_recovery_journal(
                    &planned.id,
                    RecoveryState::Planned,
                    RecoveryState::Applying,
                    json!({"stage": "applying"}),
                )
                .await?,
        )?;
        let applied_once = ModeMutationDataEffects::apply(&storage, &applying).await?;
        let applied_twice = ModeMutationDataEffects::apply(&storage, &applied_once).await?;
        assert_eq!(
            applied_once.progress["authoring_playlist_ids"],
            applied_twice.progress["authoring_playlist_ids"]
        );
        assert_eq!(
            sqlx::query_scalar::<_, i64>("SELECT COUNT(*) FROM playlists")
                .fetch_one(&storage.pool)
                .await?,
            1
        );
        assert_eq!(
            sqlx::query_scalar::<_, i64>("SELECT COUNT(*) FROM playlist_items")
                .fetch_one(&storage.pool)
                .await?,
            2
        );

        let rolling_back = applied(
            storage
                .transition_recovery_journal(
                    &applied_once.id,
                    RecoveryState::Applying,
                    RecoveryState::RollingBack,
                    applied_once.progress.clone(),
                )
                .await?,
        )?;
        ModeMutationDataEffects::rollback(&storage, &rolling_back).await?;
        ModeMutationDataEffects::rollback(&storage, &rolling_back).await?;
        assert_eq!(
            sqlx::query_scalar::<_, i64>("SELECT COUNT(*) FROM playlists")
                .fetch_one(&storage.pool)
                .await?,
            0
        );
        assert_eq!(
            sqlx::query_scalar::<_, i64>("SELECT COUNT(*) FROM playlist_items")
                .fetch_one(&storage.pool)
                .await?,
            0
        );
        storage.close().await;
        Ok(())
    }

    fn applied(
        transition: RecoveryTransition,
    ) -> Result<music_application::recovery::RecoveryJournalEntry, Box<dyn Error + Send + Sync>>
    {
        match transition {
            RecoveryTransition::Applied(entry) => Ok(entry),
            RecoveryTransition::Conflict(_) => Err("recovery transition conflicted".into()),
        }
    }
}
