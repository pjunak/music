use music_application::recovery::{
    RecoveryDomain, RecoveryFuture, RecoveryJournalDraft, RecoveryJournalEntry, RecoveryJournalId,
    RecoveryJournalRepository, RecoveryOperation, RecoveryState, RecoveryTransition,
    validate_recovery_progress,
};
use serde_json::Value;
use sqlx::Row;

use crate::{SqliteStorage, StorageError};

const MAX_UNFINISHED_JOURNALS: usize = 1_000;

impl RecoveryJournalRepository for SqliteStorage {
    fn create_recovery_journal(
        &self,
        draft: RecoveryJournalDraft,
    ) -> RecoveryFuture<'_, RecoveryJournalEntry> {
        Box::pin(async move {
            validate_recovery_progress(&draft.plan)
                .and_then(|()| validate_recovery_progress(&draft.progress))
                .map_err(StorageError::InvalidRecoveryJournal)
                .map_err(box_storage)?;
            let plan_json = serde_json::to_string(&draft.plan)
                .map_err(StorageError::RecoveryJournalSerialization)
                .map_err(box_storage)?;
            let progress_json = serde_json::to_string(&draft.progress)
                .map_err(StorageError::RecoveryJournalSerialization)
                .map_err(box_storage)?;
            let _admission = self.write_gate.lock().await;
            sqlx::query(
                "INSERT INTO recovery_journal (id, domain, operation, state, plan_json, \
                 progress_json, created_at, updated_at, completed_at) \
                 VALUES (?, ?, ?, 'planned', ?, ?, CURRENT_TIMESTAMP, CURRENT_TIMESTAMP, NULL)",
            )
            .bind(draft.id.as_str())
            .bind(draft.domain.as_str())
            .bind(draft.operation.as_str())
            .bind(plan_json)
            .bind(progress_json)
            .execute(&self.pool)
            .await
            .map_err(StorageError::from)
            .map_err(box_storage)?;
            read_entry(&self.pool, &draft.id)
                .await
                .map_err(box_storage)?
                .ok_or_else(|| box_storage(StorageError::InvalidRecoveryJournalRecord))
        })
    }

    fn unfinished_recovery_journals(
        &self,
        domain: RecoveryDomain,
    ) -> RecoveryFuture<'_, Vec<RecoveryJournalEntry>> {
        Box::pin(async move {
            let limit = i64::try_from(MAX_UNFINISHED_JOURNALS + 1)
                .map_err(|_| StorageError::RecoveryJournalCapacityExceeded)
                .map_err(box_storage)?;
            let rows = sqlx::query(
                "SELECT id, domain, operation, state, plan_json, progress_json, \
                        CAST(strftime('%s', created_at) AS INTEGER) AS created_at, \
                        CAST(strftime('%s', updated_at) AS INTEGER) AS updated_at, \
                        CAST(strftime('%s', completed_at) AS INTEGER) AS completed_at \
                 FROM recovery_journal \
                 WHERE domain = ? AND state NOT IN ('committed', 'rolled_back', 'failed') \
                 ORDER BY created_at, id LIMIT ?",
            )
            .bind(domain.as_str())
            .bind(limit)
            .fetch_all(&self.pool)
            .await
            .map_err(StorageError::from)
            .map_err(box_storage)?;
            if rows.len() > MAX_UNFINISHED_JOURNALS {
                return Err(box_storage(StorageError::RecoveryJournalCapacityExceeded));
            }
            rows.iter()
                .map(entry_from_row)
                .collect::<Result<Vec<_>, _>>()
                .map_err(box_storage)
        })
    }

    fn transition_recovery_journal<'a>(
        &'a self,
        id: &'a RecoveryJournalId,
        expected: RecoveryState,
        next: RecoveryState,
        progress: Value,
    ) -> RecoveryFuture<'a, RecoveryTransition> {
        Box::pin(async move {
            if !expected.allows(next) {
                return Err(box_storage(StorageError::InvalidRecoveryTransition));
            }
            validate_recovery_progress(&progress)
                .map_err(StorageError::InvalidRecoveryJournal)
                .map_err(box_storage)?;
            let progress_json = serde_json::to_string(&progress)
                .map_err(StorageError::RecoveryJournalSerialization)
                .map_err(box_storage)?;
            let _admission = self.write_gate.lock().await;
            let result = sqlx::query(
                "UPDATE recovery_journal SET state = ?, progress_json = ?, \
                        updated_at = CURRENT_TIMESTAMP, \
                        completed_at = CASE WHEN ? THEN CURRENT_TIMESTAMP ELSE NULL END \
                 WHERE id = ? AND state = ?",
            )
            .bind(next.as_str())
            .bind(progress_json)
            .bind(next.is_terminal())
            .bind(id.as_str())
            .bind(expected.as_str())
            .execute(&self.pool)
            .await
            .map_err(StorageError::from)
            .map_err(box_storage)?;
            let current = read_entry(&self.pool, id).await.map_err(box_storage)?;
            if result.rows_affected() == 1 {
                current
                    .map(RecoveryTransition::Applied)
                    .ok_or_else(|| box_storage(StorageError::InvalidRecoveryJournalRecord))
            } else {
                Ok(RecoveryTransition::Conflict(current))
            }
        })
    }
}

async fn read_entry(
    pool: &sqlx::SqlitePool,
    id: &RecoveryJournalId,
) -> Result<Option<RecoveryJournalEntry>, StorageError> {
    let row = sqlx::query(
        "SELECT id, domain, operation, state, plan_json, progress_json, \
                CAST(strftime('%s', created_at) AS INTEGER) AS created_at, \
                CAST(strftime('%s', updated_at) AS INTEGER) AS updated_at, \
                CAST(strftime('%s', completed_at) AS INTEGER) AS completed_at \
         FROM recovery_journal WHERE id = ?",
    )
    .bind(id.as_str())
    .fetch_optional(pool)
    .await?;
    row.as_ref().map(entry_from_row).transpose()
}

fn entry_from_row(row: &sqlx::sqlite::SqliteRow) -> Result<RecoveryJournalEntry, StorageError> {
    let id = RecoveryJournalId::parse(row.try_get::<String, _>("id")?)
        .map_err(StorageError::InvalidRecoveryJournal)?;
    let domain = RecoveryDomain::parse(&row.try_get::<String, _>("domain")?)
        .map_err(StorageError::InvalidRecoveryJournal)?;
    let operation = RecoveryOperation::parse(row.try_get::<String, _>("operation")?)
        .map_err(StorageError::InvalidRecoveryJournal)?;
    let state = RecoveryState::parse(&row.try_get::<String, _>("state")?)
        .map_err(StorageError::InvalidRecoveryJournal)?;
    let plan = serde_json::from_str(&row.try_get::<String, _>("plan_json")?)
        .map_err(StorageError::RecoveryJournalSerialization)?;
    let progress = serde_json::from_str(&row.try_get::<String, _>("progress_json")?)
        .map_err(StorageError::RecoveryJournalSerialization)?;
    let entry = RecoveryJournalEntry {
        id,
        domain,
        operation,
        state,
        plan,
        progress,
        created_at_unix_seconds: row.try_get("created_at")?,
        updated_at_unix_seconds: row.try_get("updated_at")?,
        completed_at_unix_seconds: row.try_get("completed_at")?,
    };
    entry
        .validate()
        .map_err(StorageError::InvalidRecoveryJournal)?;
    Ok(entry)
}

fn box_storage(error: StorageError) -> Box<dyn std::error::Error + Send + Sync> {
    Box::new(error)
}

#[cfg(test)]
mod tests {
    use std::error::Error;

    use music_application::recovery::{
        RecoveryDomain, RecoveryJournalDraft, RecoveryJournalRepository, RecoveryOperation,
        RecoveryState, RecoveryTransition,
    };
    use serde_json::json;
    use tempfile::tempdir;

    use crate::{SqliteStorage, SqliteStorageOptions};

    #[tokio::test]
    async fn journal_transitions_are_atomic_bounded_and_recoverable()
    -> Result<(), Box<dyn Error + Send + Sync>> {
        let directory = tempdir()?;
        let storage =
            SqliteStorage::open(SqliteStorageOptions::new(directory.path().join("app.db"))).await?;
        let draft = RecoveryJournalDraft::new(
            RecoveryDomain::Library,
            RecoveryOperation::parse("publish_upload")?,
            json!({"staged": "Uploads/.track.partial", "target": "Uploads/track.wav"}),
        )?;
        let id = draft.id.clone();
        let planned = storage.create_recovery_journal(draft).await?;
        assert_eq!(planned.state, RecoveryState::Planned);
        assert_eq!(
            storage
                .unfinished_recovery_journals(RecoveryDomain::Library)
                .await?,
            vec![planned.clone()]
        );

        let applying = storage
            .transition_recovery_journal(
                &id,
                RecoveryState::Planned,
                RecoveryState::Applying,
                json!({"published": true}),
            )
            .await?;
        assert!(matches!(
            applying,
            RecoveryTransition::Applied(ref entry) if entry.state == RecoveryState::Applying
        ));
        let conflict = storage
            .transition_recovery_journal(
                &id,
                RecoveryState::Planned,
                RecoveryState::Failed,
                json!({"error": "stale"}),
            )
            .await?;
        assert!(matches!(
            conflict,
            RecoveryTransition::Conflict(Some(ref entry))
                if entry.state == RecoveryState::Applying
        ));
        let committed = storage
            .transition_recovery_journal(
                &id,
                RecoveryState::Applying,
                RecoveryState::Committed,
                json!({"published": true, "indexed": true}),
            )
            .await?;
        assert!(matches!(
            committed,
            RecoveryTransition::Applied(ref entry)
                if entry.state == RecoveryState::Committed
                    && entry.completed_at_unix_seconds.is_some()
        ));
        assert!(
            storage
                .unfinished_recovery_journals(RecoveryDomain::Library)
                .await?
                .is_empty()
        );
        Ok(())
    }
}
