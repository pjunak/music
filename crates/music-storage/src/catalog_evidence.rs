use music_application::assistant::CATALOG_TAG_ANALYZER_ID;
use sqlx::{Sqlite, Transaction};

pub(crate) async fn revision(
    transaction: &mut Transaction<'_, Sqlite>,
) -> Result<i64, sqlx::Error> {
    sqlx::query_scalar("SELECT revision FROM catalog_evidence_state WHERE id = 1")
        .fetch_one(&mut **transaction)
        .await
}

/// Invalidate regenerable evidence atomically with the setting that changed.
pub(crate) async fn invalidate(
    transaction: &mut Transaction<'_, Sqlite>,
) -> Result<(), sqlx::Error> {
    sqlx::query("UPDATE catalog_evidence_state SET revision = revision + 1 WHERE id = 1")
        .execute(&mut **transaction)
        .await?;
    sqlx::query("DELETE FROM cleanup_track_enrichments")
        .execute(&mut **transaction)
        .await?;
    sqlx::query("DELETE FROM track_analysis_tag_reviews WHERE analyzer_id = ?")
        .bind(CATALOG_TAG_ANALYZER_ID)
        .execute(&mut **transaction)
        .await?;
    sqlx::query("DELETE FROM track_analyses WHERE analyzer_id = ?")
        .bind(CATALOG_TAG_ANALYZER_ID)
        .execute(&mut **transaction)
        .await?;
    Ok(())
}

pub(crate) async fn song_catalog_rows(
    connection: &mut sqlx::SqliteConnection,
    track_id: Option<i64>,
) -> Result<Vec<sqlx::sqlite::SqliteRow>, sqlx::Error> {
    sqlx::query("SELECT e.track_id, e.source_signature, e.evidence_revision, e.result_json,
        COALESCE((SELECT enabled FROM cleanup_source_policies WHERE source_id='lastfm'),0) AS lastfm_enabled
        FROM cleanup_track_enrichments e
        WHERE e.evidence_revision=(SELECT revision FROM catalog_evidence_state WHERE id=1)
        AND COALESCE((SELECT enabled FROM cleanup_source_policies WHERE source_id='musicbrainz'),1)=1
        AND (? IS NULL OR e.track_id=?)")
        .bind(track_id).bind(track_id).fetch_all(connection).await
}

pub(crate) fn song_catalog_projection(
    row: &sqlx::sqlite::SqliteRow,
    track: &music_domain::IndexedTrack,
) -> Option<serde_json::Value> {
    use sqlx::Row;
    let result: &str = row.try_get("result_json").ok()?;
    if result.len() > 1024 * 1024 {
        return None;
    }
    let record = music_application::cleanup_enrichment::CleanupEnrichmentRecord {
        track_id: music_domain::TrackId::new(row.try_get("track_id").ok()?).ok()?,
        evidence_revision: row.try_get("evidence_revision").ok()?,
        source_signature: row.try_get("source_signature").ok()?,
        result: serde_json::from_str(result).ok()?,
    };
    music_application::assistant::song_catalog_evidence(
        track,
        &record,
        true,
        row.try_get("lastfm_enabled").ok()?,
    )
}

pub(crate) async fn current_song_catalog(
    transaction: &mut Transaction<'_, Sqlite>,
    track: &music_domain::IndexedTrack,
) -> Result<Option<serde_json::Value>, sqlx::Error> {
    let rows = song_catalog_rows(transaction, Some(track.id.get())).await?;
    Ok(rows
        .first()
        .and_then(|row| song_catalog_projection(row, track)))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{SqliteStorage, SqliteStorageOptions};
    use music_application::assistant::{AssistantRepository, model_tag_source_signature};
    use music_application::cleanup_enrichment::{
        CleanupEnrichmentRecord, CleanupEnrichmentRepository, cleanup_enrichment_source_signature,
    };
    use music_application::cleanup_sources::CleanupSourceRepository;
    use music_application::library::LibraryRepository;
    use music_domain::TrackId;
    use serde_json::json;

    #[tokio::test]
    async fn tagging_catalog_claims_follow_policy_and_invalidate_results_without_touching_authored_tags()
    -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
        let directory = tempfile::tempdir()?;
        let storage =
            SqliteStorage::open(SqliteStorageOptions::new(directory.path().join("app.db"))).await?;
        sqlx::query("INSERT INTO tracks (id,path,title,artist,album_artist,album,genre,length_s,display_title,origin,size_bytes,mtime,added_at) VALUES (1,'private.flac','Private title','Artist','','Album','',120,'Private display','',10,20,CURRENT_TIMESTAMP)").execute(&storage.pool).await?;
        sqlx::query("INSERT INTO track_user_tags VALUES (1,'kept',CURRENT_TIMESTAMP)")
            .execute(&storage.pool)
            .await?;
        storage.set_cleanup_source_enabled("lastfm", true).await?;
        let track = storage
            .track(TrackId::new(1)?)
            .await?
            .ok_or("missing track")?;
        let revision = storage.catalog_evidence_revision().await?;
        let record = CleanupEnrichmentRecord {
            track_id: track.id,
            evidence_revision: revision,
            source_signature: cleanup_enrichment_source_signature(&track)?,
            result: serde_json::from_value(
                json!({"status":"identified","retrieved_at":1800000000,
                "identity":{"recording_mbid":"recording-1", "title":"Excluded catalog title"},
                "recording_observations":{"genres":["chamber music"],"composers":["Composer"],"first_release_date":"1999-01"},
                "community_observations":{"status":"available","recording_mbid":"recording-1","evidence_revision":revision,
                    "policy_contract":music_application::cleanup_enrichment::CATALOG_EVIDENCE_POLICY_CONTRACT,
                    "retrieved_at":1800000000,"tags":[{"name":"Unmapped atmosphere","count":12}]},
                "tag_suggestions":[{"tag":"do not feed generated tags back"}]}),
            )?,
        };
        assert!(storage.store_cleanup_enrichment(&record).await?);
        let evidence = storage.tracks().await?;
        let catalog = evidence[0]
            .catalog_evidence
            .as_ref()
            .ok_or("catalog missing")?;
        assert_eq!(
            catalog["claims"].as_array().ok_or("claims missing")?.len(),
            4
        );
        let serialized = catalog.to_string();
        for excluded in [
            "Private title",
            "Private display",
            "private.flac",
            "Excluded catalog title",
            "do not feed generated tags back",
            "kept",
        ] {
            assert!(!serialized.contains(excluded));
        }
        assert!(serialized.contains("weak_community_labels"));
        let before = model_tag_source_signature(&track, "role", "vocabulary", None, Some(catalog))?;
        let mut transaction = storage.pool.begin().await?;
        assert_eq!(
            current_song_catalog(&mut transaction, &track)
                .await?
                .as_ref(),
            Some(catalog)
        );
        transaction.rollback().await?;
        storage.set_cleanup_source_enabled("lastfm", false).await?;
        let evidence = storage.tracks().await?;
        assert!(evidence[0].catalog_evidence.is_none());
        assert_eq!(evidence[0].manual_tags, ["kept"]);
        assert_ne!(
            before,
            model_tag_source_signature(
                &track,
                "role",
                "vocabulary",
                None,
                evidence[0].catalog_evidence.as_ref()
            )?
        );
        assert!(!storage.store_cleanup_enrichment(&record).await?);
        Ok(())
    }
}
