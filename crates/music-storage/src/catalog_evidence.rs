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
