use crate::library::{TRACK_COLUMNS, indexed_track_from_row};
use crate::{SqliteStorage, StorageError};
use music_application::cleanup::CleanupFuture;
use music_application::cleanup::rejections::{
    CleanupRejection, CleanupRejectionRepository, CleanupReviewProposal, REJECTION_PAGE_SIZE,
    RejectionSnapshot, rejection_context,
};
use sqlx::{AssertSqlSafe, Row, Sqlite, Transaction};
use std::collections::BTreeMap;

async fn snapshot(tx: &mut Transaction<'_, Sqlite>) -> Result<RejectionSnapshot, StorageError> {
    let rows = sqlx::query(AssertSqlSafe(format!(
        "SELECT {TRACK_COLUMNS} FROM tracks ORDER BY path"
    )))
    .fetch_all(&mut **tx)
    .await?;
    let tracks = rows
        .iter()
        .map(indexed_track_from_row)
        .collect::<Result<Vec<_>, _>>()?;
    let catalog_revision =
        sqlx::query_scalar("SELECT revision FROM catalog_evidence_state WHERE id = 1")
            .fetch_one(&mut **tx)
            .await?;
    Ok(RejectionSnapshot {
        tracks,
        catalog_revision,
    })
}

fn record(row: &sqlx::sqlite::SqliteRow) -> Result<CleanupRejection, StorageError> {
    Ok(CleanupRejection {
        id: row.try_get("id")?,
        fingerprint: row.try_get("fingerprint")?,
        context_signature: row.try_get("context_signature")?,
        proposal: serde_json::from_str(row.try_get::<&str, _>("proposal_json")?)
            .map_err(StorageError::AssistantSerialization)?,
        rejected_at: row.try_get("rejected_at")?,
    })
}

impl CleanupRejectionRepository for SqliteStorage {
    fn rejection_snapshot(&self) -> CleanupFuture<'_, RejectionSnapshot> {
        Box::pin(async move {
            let mut tx = self.pool.begin().await?;
            Ok(snapshot(&mut tx).await?)
        })
    }

    fn match_rejections<'a>(
        &'a self,
        fingerprints: &'a [String],
    ) -> CleanupFuture<'a, Vec<Option<i64>>> {
        Box::pin(async move {
            let rows = sqlx::query_as::<_, (String, i64)>("SELECT fingerprint, id FROM cleanup_rejections WHERE fingerprint IN (SELECT value FROM json_each(?))")
                .bind(serde_json::to_string(fingerprints)?).fetch_all(&self.pool).await?;
            let matches = rows.into_iter().collect::<BTreeMap<_, _>>();
            Ok(fingerprints
                .iter()
                .map(|key| matches.get(key).copied())
                .collect())
        })
    }

    fn save_rejection<'a>(
        &'a self,
        proposal: &'a CleanupReviewProposal,
        context: &'a str,
    ) -> CleanupFuture<'a, Option<CleanupRejection>> {
        Box::pin(async move {
            let _admission = self.write_gate.lock().await;
            let mut tx = self.pool.begin().await?;
            if proposal.validate().is_err()
                || rejection_context(proposal, &snapshot(&mut tx).await?).as_deref()
                    != Some(context)
            {
                return Ok(None);
            }
            let fingerprint = proposal.fingerprint(context);
            let search = format!(
                "{} {} {} {}",
                proposal.path,
                proposal.field.as_deref().unwrap_or(&proposal.kind),
                proposal.old,
                proposal.new
            )
            .to_lowercase();
            sqlx::query("INSERT INTO cleanup_rejections (fingerprint, context_signature, proposal_json, search_text) VALUES (?, ?, ?, ?) ON CONFLICT(fingerprint) DO NOTHING")
                .bind(&fingerprint).bind(context).bind(serde_json::to_string(proposal)?).bind(search).execute(&mut *tx).await?;
            let row = sqlx::query("SELECT * FROM cleanup_rejections WHERE fingerprint = ?")
                .bind(fingerprint)
                .fetch_one(&mut *tx)
                .await?;
            let record = record(&row)?;
            tx.commit().await?;
            Ok(Some(record))
        })
    }

    fn rejection_page<'a>(
        &'a self,
        before: Option<i64>,
        search: &'a str,
    ) -> CleanupFuture<'a, Vec<CleanupRejection>> {
        Box::pin(async move {
            let rows = sqlx::query("SELECT * FROM cleanup_rejections WHERE (? IS NULL OR id < ?) AND instr(search_text, ?) > 0 ORDER BY id DESC LIMIT ?")
                .bind(before).bind(before).bind(search.to_lowercase()).bind(i64::try_from(REJECTION_PAGE_SIZE + 1)?).fetch_all(&self.pool).await?;
            Ok(rows.iter().map(record).collect::<Result<Vec<_>, _>>()?)
        })
    }

    fn restore_rejection(&self, id: i64) -> CleanupFuture<'_, Option<CleanupRejection>> {
        Box::pin(async move {
            let _admission = self.write_gate.lock().await;
            let mut tx = self.pool.begin().await?;
            let Some(row) = sqlx::query("SELECT * FROM cleanup_rejections WHERE id = ?")
                .bind(id)
                .fetch_optional(&mut *tx)
                .await?
            else {
                return Ok(None);
            };
            let record = record(&row)?;
            if rejection_context(&record.proposal, &snapshot(&mut tx).await?).as_deref()
                != Some(&record.context_signature)
            {
                return Ok(None);
            }
            sqlx::query("DELETE FROM cleanup_rejections WHERE id = ?")
                .bind(id)
                .execute(&mut *tx)
                .await?;
            tx.commit().await?;
            Ok(Some(record))
        })
    }

    fn forget_rejection(&self, id: i64) -> CleanupFuture<'_, ()> {
        Box::pin(async move {
            let _admission = self.write_gate.lock().await;
            sqlx::query("DELETE FROM cleanup_rejections WHERE id = ?")
                .bind(id)
                .execute(&self.pool)
                .await?;
            Ok(())
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use music_application::cleanup::{CleanupService, rejections::RejectionError};
    use serde_json::json;
    use std::sync::Arc;
    type TestResult = Result<(), Box<dyn std::error::Error + Send + Sync>>;

    async fn seed(storage: &SqliteStorage) -> TestResult {
        sqlx::query("INSERT INTO tracks (id,path,title,artist,album_artist,album,track_no,disc_no,year,genre,length_s,bpm,size_bytes,mtime,added_at,display_title,origin) VALUES (1,'Album/Disc 1/Song.mp3','Song','Artist','','Album',1,1,2026,'',120,NULL,10,20,CURRENT_TIMESTAMP,'',''), (2,'Album/Disc 2/Finale.mp3','Finale','Artist','','Album',1,2,2026,'',120,NULL,10,20,CURRENT_TIMESTAMP,'','')").execute(&storage.pool).await?;
        Ok(())
    }
    fn proposal() -> CleanupReviewProposal {
        CleanupReviewProposal {
            op_id: "catalog:1:title:job-1".into(),
            track_id: 1,
            path: "Album/Disc 1/Song.mp3".into(),
            kind: "tag".into(),
            field: Some("title".into()),
            old: json!("Song"),
            new: json!("Opening"),
            rules: vec!["catalog_identity".into()],
            confidence: "low".into(),
            verified: false,
            evidence: Some(
                json!({"source":"musicbrainz","entity":"recording","id":"00000000-0000-0000-0000-000000000001"}),
            ),
            evidence_context: Some("independent observations".into()),
        }
    }

    #[tokio::test]
    async fn rejected_proposals_survive_restart_and_restore_without_writing_metadata() -> TestResult
    {
        let dir = tempfile::tempdir()?;
        let path = dir.path().join("app.db");
        let storage = Arc::new(SqliteStorage::open(crate::SqliteStorageOptions::new(&path)).await?);
        seed(&storage).await?;
        let service = CleanupService::new(storage.clone());
        let mut proposal = proposal();
        let first = service.reject_proposal(proposal.clone()).await?;
        proposal.op_id = "catalog:1:title:another-job".into();
        assert_eq!(
            service.reject_proposal(proposal.clone()).await?.id,
            first.id
        );
        assert_eq!(
            service.rejected_matches(&[proposal.clone()]).await?,
            vec![Some(first.id)]
        );
        storage.close().await;
        drop(service);
        drop(storage);
        let storage = Arc::new(SqliteStorage::open(crate::SqliteStorageOptions::new(&path)).await?);
        let service = CleanupService::new(storage.clone());
        let pool = service.rejected_page(None, "opening").await?;
        assert_eq!(pool.len(), 1);
        assert!(pool[0].1);
        assert_eq!(
            service.restore_rejected(first.id).await?.new,
            json!("Opening")
        );
        assert_eq!(service.rejected_matches(&[proposal]).await?, vec![None]);
        let title: String = sqlx::query_scalar("SELECT title FROM tracks WHERE id = 1")
            .fetch_one(&storage.pool)
            .await?;
        assert_eq!(title, "Song");
        assert!(service.rejected_page(None, "").await?.is_empty());
        Ok(())
    }

    #[tokio::test]
    async fn rejection_review_preserves_raw_whitespace_in_existing_tags() -> TestResult {
        let dir = tempfile::tempdir()?;
        let storage = Arc::new(
            SqliteStorage::open(crate::SqliteStorageOptions::new(dir.path().join("app.db")))
                .await?,
        );
        seed(&storage).await?;
        let raw_title = "Opening\t Theme\r\n";
        sqlx::query("UPDATE tracks SET title = ? WHERE id = 1")
            .bind(raw_title)
            .execute(&storage.pool)
            .await?;
        let service = CleanupService::new(storage);
        let mut proposal = proposal();
        proposal.old = json!(raw_title);
        proposal.new = json!("Opening Theme");
        proposal.rules = vec!["tag_title".into()];
        proposal.evidence = None;
        proposal.evidence_context = None;
        assert_eq!(
            service
                .rejected_matches(std::slice::from_ref(&proposal))
                .await?,
            vec![None]
        );
        let rejected = service.reject_proposal(proposal.clone()).await?;
        assert_eq!(
            service
                .rejected_matches(std::slice::from_ref(&proposal))
                .await?,
            vec![Some(rejected.id)]
        );
        assert_eq!(service.restore_rejected(rejected.id).await?, proposal);
        Ok(())
    }

    #[tokio::test]
    async fn changed_evidence_resurfaces_proposals_while_old_rejections_remain_accessible()
    -> TestResult {
        let dir = tempfile::tempdir()?;
        let storage = Arc::new(
            SqliteStorage::open(crate::SqliteStorageOptions::new(dir.path().join("app.db")))
                .await?,
        );
        seed(&storage).await?;
        let service = CleanupService::new(storage.clone());
        let original = proposal();
        let first = service.reject_proposal(original.clone()).await?;
        let mut new_context = original.clone();
        new_context.evidence_context = Some("new local observations".into());
        let mut new_catalog = original.clone();
        new_catalog.evidence = Some(
            json!({"source":"musicbrainz","entity":"recording","id":"00000000-0000-0000-0000-000000000002"}),
        );
        for changed in [new_context, new_catalog] {
            assert_eq!(service.rejected_matches(&[changed]).await?, vec![None]);
        }
        sqlx::query("UPDATE tracks SET artist = 'Another Composer' WHERE id = 2")
            .execute(&storage.pool)
            .await?;
        assert_eq!(
            service
                .rejected_matches(std::slice::from_ref(&original))
                .await?,
            vec![None]
        );
        assert!(!service.rejected_page(None, "").await?[0].1);
        assert!(matches!(
            service.restore_rejected(first.id).await,
            Err(RejectionError::Stale)
        ));
        assert!(
            storage
                .save_rejection(&original, &first.context_signature)
                .await?
                .is_none()
        );
        let second = service.reject_proposal(original.clone()).await?;
        assert_ne!(first.id, second.id);
        sqlx::query("UPDATE catalog_evidence_state SET revision = revision + 1 WHERE id = 1")
            .execute(&storage.pool)
            .await?;
        assert_eq!(
            service
                .rejected_matches(std::slice::from_ref(&original))
                .await?,
            vec![None]
        );
        assert!(matches!(
            service.restore_rejected(second.id).await,
            Err(RejectionError::Stale)
        ));
        assert_eq!(service.rejected_page(None, "").await?.len(), 2);
        service.forget_rejected(first.id).await?;
        assert_eq!(service.rejected_page(None, "").await?.len(), 1);
        sqlx::query("DELETE FROM tracks WHERE id = 1")
            .execute(&storage.pool)
            .await?;
        assert!(!service.rejected_page(None, "").await?[0].1);
        assert!(matches!(
            service.reject_proposal(original).await,
            Err(RejectionError::Stale)
        ));
        Ok(())
    }

    #[tokio::test]
    async fn rejection_pool_pages_searches_literal_text_and_handles_folder_and_file_names()
    -> TestResult {
        let dir = tempfile::tempdir()?;
        let storage = Arc::new(
            SqliteStorage::open(crate::SqliteStorageOptions::new(dir.path().join("app.db")))
                .await?,
        );
        seed(&storage).await?;
        let service = CleanupService::new(storage.clone());
        for i in 0..53 {
            let mut p = proposal();
            p.new = json!(format!("Scene_{i}%"));
            service.reject_proposal(p).await?;
        }
        let page = service.rejected_page(None, "scene_").await?;
        assert_eq!(page.len(), REJECTION_PAGE_SIZE + 1);
        let older = service
            .rejected_page(Some(page[REJECTION_PAGE_SIZE - 1].0.id), "scene_")
            .await?;
        assert_eq!(older.len(), 3);
        assert!(
            older
                .iter()
                .all(|(row, _)| row.id < page[REJECTION_PAGE_SIZE - 1].0.id)
        );
        assert_eq!(service.rejected_page(None, "_0%").await?.len(), 1);
        let mut rename = proposal();
        rename.kind = "rename".into();
        rename.field = None;
        rename.evidence = None;
        let rename_id = service.reject_proposal(rename.clone()).await?.id;
        assert_eq!(service.restore_rejected(rename_id).await?.kind, "rename");
        rename.kind = "folder_rename".into();
        rename.track_id = 0;
        rename.path = "Album".into();
        rename.old = json!("Album");
        let folder = service.reject_proposal(rename.clone()).await?;
        sqlx::query("UPDATE tracks SET path = 'Other/Disc 2/Finale.mp3' WHERE id = 2")
            .execute(&storage.pool)
            .await?;
        assert!(matches!(
            service.restore_rejected(folder.id).await,
            Err(RejectionError::Stale)
        ));
        rename.new = json!("../escape");
        assert!(matches!(
            service.reject_proposal(rename).await,
            Err(RejectionError::Invalid)
        ));
        assert!(matches!(
            service.rejected_matches(&vec![proposal(); 101]).await,
            Err(RejectionError::Invalid)
        ));
        Ok(())
    }

    #[tokio::test]
    async fn schema_twelve_upgrades_with_backup_and_preserves_indexed_metadata() -> TestResult {
        let dir = tempfile::tempdir()?;
        let path = dir.path().join("app.db");
        let storage = SqliteStorage::open(crate::SqliteStorageOptions::new(&path)).await?;
        seed(&storage).await?;
        // Build the exact previous schema in this disposable fixture database.
        sqlx::query("DROP TABLE cleanup_rejections")
            .execute(&storage.pool)
            .await?;
        sqlx::raw_sql("ALTER TABLE tracks DROP COLUMN release_date; ALTER TABLE tracks DROP COLUMN original_release_date; ALTER TABLE tracks DROP COLUMN composer;").execute(&storage.pool).await?;
        sqlx::query("DROP TABLE track_contexts")
            .execute(&storage.pool)
            .await?;
        sqlx::raw_sql(include_str!("../migrations/0001_rust_baseline.sql"))
            .execute(&storage.pool)
            .await?;
        sqlx::query("DELETE FROM _sqlx_migrations WHERE version >= 13")
            .execute(&storage.pool)
            .await?;
        storage.close().await;
        drop(storage);
        let storage = SqliteStorage::open(crate::SqliteStorageOptions::new(&path)).await?;
        assert!(storage.migration_outcome().backup.is_some());
        assert_eq!(
            storage.migration_outcome().schema_after.migration_version,
            Some(crate::CURRENT_SCHEMA_VERSION)
        );
        let count: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM tracks WHERE album = 'Album'")
            .fetch_one(&storage.pool)
            .await?;
        assert_eq!(count, 2);
        Ok(())
    }
}
