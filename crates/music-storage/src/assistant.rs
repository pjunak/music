use std::collections::{BTreeMap, BTreeSet};

use music_application::assistant::{
    AnalysisReviewBatch, AnalysisReviewDecision, AnalysisReviewFailure, AnalysisReviewFailureCode,
    AnalysisReviewOutcome, AnalysisReviewTarget, AssistantFuture, AssistantRepository,
    AssistantTrackEvidence, BulkTagFailure, BulkTagOutcome, CATALOG_TAG_ANALYZER_ID,
    CleanupApplyOutcome, CleanupMutation, CleanupSelection, LOCAL_METADATA_ANALYZER_ID,
    MAX_TAGS_PER_TRACK, RenameTagOutcome, StoredAnalysis, StoredAnalysisReview, TagUsage,
    TagVocabularyDocument, TagVocabularyRecord, TagVocabularySnapshot, build_cleanup_preview,
    catalog_signature, metadata_source_signature, vocabulary_fingerprint,
};
use music_domain::TrackId;
use serde_json::{Map, Value};
use sqlx::{AssertSqlSafe, QueryBuilder, Row, Sqlite, Transaction};

use crate::library::{TRACK_COLUMNS, indexed_track_from_row};
use crate::{SqliteStorage, StorageError};

const VOCABULARY_KEY: &str = "library";

impl AssistantRepository for SqliteStorage {
    fn tracks(&self) -> AssistantFuture<'_, Vec<AssistantTrackEvidence>> {
        Box::pin(async move {
            let query = format!("SELECT {TRACK_COLUMNS} FROM tracks ORDER BY id");
            let rows = sqlx::query(AssertSqlSafe(query))
                .fetch_all(&self.pool)
                .await
                .map_err(box_storage)?;
            let tracks = rows
                .iter()
                .map(indexed_track_from_row)
                .collect::<Result<Vec<_>, _>>()?;
            let mut manual = BTreeMap::<i64, Vec<String>>::new();
            for row in
                sqlx::query("SELECT track_id, tag FROM track_user_tags ORDER BY track_id, tag")
                    .fetch_all(&self.pool)
                    .await
                    .map_err(box_storage)?
            {
                manual
                    .entry(row.try_get("track_id").map_err(box_storage)?)
                    .or_default()
                    .push(row.try_get("tag").map_err(box_storage)?);
            }
            let mut analyses = BTreeMap::<i64, Vec<StoredAnalysis>>::new();
            for row in sqlx::query(
                "SELECT track_id, analyzer_id, source_signature, energy, brightness, tension, \
                 moods_json, evidence_json, metrics_json, confidence FROM track_analyses \
                 ORDER BY track_id, analyzer_id",
            )
            .fetch_all(&self.pool)
            .await
            .map_err(box_storage)?
            {
                let Some(analysis) = analysis_from_row(&row) else {
                    continue;
                };
                analyses
                    .entry(row.try_get("track_id").map_err(box_storage)?)
                    .or_default()
                    .push(analysis);
            }
            let mut reviews = BTreeMap::<i64, Vec<StoredAnalysisReview>>::new();
            for row in sqlx::query(
                "SELECT track_id, analyzer_id, source_signature, tag, decision \
                 FROM track_analysis_tag_reviews ORDER BY track_id, analyzer_id, tag",
            )
            .fetch_all(&self.pool)
            .await
            .map_err(box_storage)?
            {
                let decision: String = row.try_get("decision").map_err(box_storage)?;
                let Some(decision) = AnalysisReviewDecision::parse(&decision) else {
                    continue;
                };
                reviews
                    .entry(row.try_get("track_id").map_err(box_storage)?)
                    .or_default()
                    .push(StoredAnalysisReview {
                        analyzer_id: row.try_get("analyzer_id").map_err(box_storage)?,
                        source_signature: row.try_get("source_signature").map_err(box_storage)?,
                        tag: row.try_get("tag").map_err(box_storage)?,
                        decision,
                    });
            }
            Ok(tracks
                .into_iter()
                .map(|track| {
                    let track_id = track.id.get();
                    AssistantTrackEvidence {
                        track,
                        manual_tags: manual.remove(&track_id).unwrap_or_default(),
                        analyses: analyses.remove(&track_id).unwrap_or_default(),
                        reviews: reviews.remove(&track_id).unwrap_or_default(),
                    }
                })
                .collect())
        })
    }

    fn vocabulary(&self) -> AssistantFuture<'_, Option<TagVocabularyRecord>> {
        Box::pin(async move { load_vocabulary(&self.pool).await.map_err(box_storage) })
    }

    fn initialize_vocabulary<'a>(
        &'a self,
        record: &'a TagVocabularyRecord,
    ) -> AssistantFuture<'a, TagVocabularyRecord> {
        Box::pin(async move {
            let document = serde_json::to_string(&record.document)
                .map_err(StorageError::AssistantSerialization)
                .map_err(box_storage)?;
            let revision = i64::from(record.revision);
            let seed_version = i64::from(record.seed_version);
            let _admission = self.write_gate.lock().await;
            sqlx::query(
                "INSERT OR IGNORE INTO assistant_tag_vocabularies \
                 (\"key\", revision, seed_version, document_json, updated_at) \
                 VALUES (?, ?, ?, ?, CURRENT_TIMESTAMP)",
            )
            .bind(VOCABULARY_KEY)
            .bind(revision)
            .bind(seed_version)
            .bind(document)
            .execute(&self.pool)
            .await
            .map_err(box_storage)?;
            load_vocabulary(&self.pool)
                .await
                .map_err(box_storage)?
                .ok_or_else(|| {
                    box_storage(StorageError::InvalidAssistantRecord(
                        "vocabulary insert disappeared",
                    ))
                })
        })
    }

    fn replace_vocabulary<'a>(
        &'a self,
        expected_revision: u32,
        document: &'a TagVocabularyDocument,
    ) -> AssistantFuture<'a, Option<TagVocabularyRecord>> {
        Box::pin(async move {
            let encoded = serde_json::to_string(document)
                .map_err(StorageError::AssistantSerialization)
                .map_err(box_storage)?;
            let next_revision = expected_revision.checked_add(1).ok_or_else(|| {
                box_storage(StorageError::InvalidAssistantRecord(
                    "vocabulary revision overflow",
                ))
            })?;
            let _admission = self.write_gate.lock().await;
            let changed = sqlx::query(
                "UPDATE assistant_tag_vocabularies SET document_json = ?, revision = ?, \
                 updated_at = CURRENT_TIMESTAMP WHERE \"key\" = ? AND revision = ?",
            )
            .bind(encoded)
            .bind(i64::from(next_revision))
            .bind(VOCABULARY_KEY)
            .bind(i64::from(expected_revision))
            .execute(&self.pool)
            .await
            .map_err(box_storage)?
            .rows_affected();
            if changed != 1 {
                return Ok(None);
            }
            load_vocabulary(&self.pool).await.map_err(box_storage)
        })
    }

    fn patch_tags<'a>(
        &'a self,
        track_ids: &'a [TrackId],
        add: &'a [String],
        remove: &'a [String],
    ) -> AssistantFuture<'a, BulkTagOutcome> {
        Box::pin(async move {
            let requested = track_ids.iter().copied().collect::<BTreeSet<_>>();
            let requested_raw = requested.iter().map(|id| id.get()).collect::<Vec<_>>();
            let _admission = self.write_gate.lock().await;
            let mut transaction = self.pool.begin().await.map_err(box_storage)?;
            let existing = existing_track_ids(&mut transaction, &requested_raw).await?;
            let mut current = load_manual_tags(&mut transaction, &existing).await?;
            let add = add.iter().cloned().collect::<BTreeSet<_>>();
            let remove = remove.iter().cloned().collect::<BTreeSet<_>>();
            let mut changed = Vec::new();
            let mut failures = Vec::new();
            for track_id in &existing {
                let before = current.remove(track_id).unwrap_or_default();
                let after = before
                    .difference(&remove)
                    .cloned()
                    .chain(add.iter().cloned())
                    .collect::<BTreeSet<_>>();
                let domain_id = TrackId::new(*track_id).map_err(|_| {
                    box_storage(StorageError::InvalidAssistantRecord("invalid track id"))
                })?;
                if after.len() > MAX_TAGS_PER_TRACK {
                    failures.push(BulkTagFailure {
                        track_id: domain_id,
                        error: format!("track would exceed the {MAX_TAGS_PER_TRACK}-tag limit"),
                    });
                    continue;
                }
                if after == before {
                    continue;
                }
                for tag in before.difference(&after) {
                    sqlx::query("DELETE FROM track_user_tags WHERE track_id = ? AND tag = ?")
                        .bind(track_id)
                        .bind(tag)
                        .execute(&mut *transaction)
                        .await
                        .map_err(box_storage)?;
                }
                for tag in after.difference(&before) {
                    sqlx::query(
                        "INSERT INTO track_user_tags (track_id, tag, created_at) \
                         VALUES (?, ?, CURRENT_TIMESTAMP)",
                    )
                    .bind(track_id)
                    .bind(tag)
                    .execute(&mut *transaction)
                    .await
                    .map_err(box_storage)?;
                }
                changed.push(domain_id);
            }
            transaction.commit().await.map_err(box_storage)?;
            let missing = requested
                .iter()
                .filter(|id| !existing.contains(&id.get()))
                .copied()
                .collect::<Vec<_>>();
            Ok(BulkTagOutcome {
                requested_tracks: requested.len(),
                matched_tracks: existing.len(),
                changed_track_ids: changed,
                missing_track_ids: missing,
                failures,
            })
        })
    }

    fn rename_tag<'a>(
        &'a self,
        source: &'a str,
        target: &'a str,
    ) -> AssistantFuture<'a, Option<RenameTagOutcome>> {
        Box::pin(async move {
            let _admission = self.write_gate.lock().await;
            let mut transaction = self.pool.begin().await.map_err(box_storage)?;
            let affected: i64 =
                sqlx::query_scalar("SELECT COUNT(*) FROM track_user_tags WHERE tag = ?")
                    .bind(source)
                    .fetch_one(&mut *transaction)
                    .await
                    .map_err(box_storage)?;
            if affected == 0 {
                transaction.commit().await.map_err(box_storage)?;
                return Ok(None);
            }
            let merged: bool = sqlx::query_scalar::<_, i64>(
                "SELECT EXISTS(SELECT 1 FROM track_user_tags WHERE tag = ?)",
            )
            .bind(target)
            .fetch_one(&mut *transaction)
            .await
            .map_err(box_storage)?
                != 0;
            sqlx::query(
                "INSERT OR IGNORE INTO track_user_tags (track_id, tag, created_at) \
                 SELECT track_id, ?, CURRENT_TIMESTAMP FROM track_user_tags WHERE tag = ?",
            )
            .bind(target)
            .bind(source)
            .execute(&mut *transaction)
            .await
            .map_err(box_storage)?;
            sqlx::query("DELETE FROM track_user_tags WHERE tag = ?")
                .bind(source)
                .execute(&mut *transaction)
                .await
                .map_err(box_storage)?;
            transaction.commit().await.map_err(box_storage)?;
            Ok(Some(RenameTagOutcome {
                source: source.to_owned(),
                target: target.to_owned(),
                affected_tracks: usize::try_from(affected).map_err(|_| {
                    box_storage(StorageError::InvalidAssistantRecord("tag count overflow"))
                })?,
                merged,
            }))
        })
    }

    fn apply_cleanup<'a>(
        &'a self,
        expected_catalog_signature: &'a str,
        expected_vocabulary_fingerprint: &'a str,
        selections: &'a [CleanupSelection],
        allowed_pairs: Option<&'a [CleanupSelection]>,
    ) -> AssistantFuture<'a, CleanupMutation> {
        Box::pin(async move {
            let _admission = self.write_gate.lock().await;
            let mut transaction = self.pool.begin().await.map_err(box_storage)?;
            let usage = load_tag_usage(&mut transaction).await?;
            let Some(vocabulary_record) = load_vocabulary_tx(&mut transaction).await? else {
                transaction.commit().await.map_err(box_storage)?;
                return Ok(CleanupMutation::StaleVocabulary);
            };
            let document = vocabulary_record.document.normalized().map_err(|_| {
                box_storage(StorageError::InvalidAssistantRecord("invalid vocabulary"))
            })?;
            let fingerprint = vocabulary_fingerprint(&document).map_err(|_| {
                box_storage(StorageError::InvalidAssistantRecord("invalid vocabulary"))
            })?;
            if fingerprint != expected_vocabulary_fingerprint {
                transaction.commit().await.map_err(box_storage)?;
                return Ok(CleanupMutation::StaleVocabulary);
            }
            let current_catalog_signature = catalog_signature(&usage).map_err(|_| {
                box_storage(StorageError::InvalidAssistantRecord(
                    "invalid catalog signature",
                ))
            })?;
            if current_catalog_signature != expected_catalog_signature {
                transaction.commit().await.map_err(box_storage)?;
                return Ok(CleanupMutation::StaleCatalog);
            }
            let allowed = if let Some(allowed_pairs) = allowed_pairs {
                allowed_pairs
                    .iter()
                    .map(|item| (item.source.clone(), item.target.clone()))
                    .collect::<BTreeSet<_>>()
            } else {
                build_cleanup_preview(
                    &usage,
                    &TagVocabularySnapshot {
                        revision: vocabulary_record.revision,
                        fingerprint,
                        document,
                    },
                )
                .map_err(|_| {
                    box_storage(StorageError::InvalidAssistantRecord(
                        "invalid cleanup preview",
                    ))
                })?
                .suggestions
                .into_iter()
                .map(|item| (item.source, item.target))
                .collect::<BTreeSet<_>>()
            };
            if selections
                .iter()
                .any(|item| !allowed.contains(&(item.source.clone(), item.target.clone())))
            {
                transaction.commit().await.map_err(box_storage)?;
                return Ok(CleanupMutation::InvalidSelection);
            }
            let mut applied = Vec::new();
            for item in selections {
                let affected: i64 =
                    sqlx::query_scalar("SELECT COUNT(*) FROM track_user_tags WHERE tag = ?")
                        .bind(&item.source)
                        .fetch_one(&mut *transaction)
                        .await
                        .map_err(box_storage)?;
                if affected == 0 {
                    transaction.rollback().await.map_err(box_storage)?;
                    return Ok(CleanupMutation::StaleCatalog);
                }
                let merged = sqlx::query_scalar::<_, i64>(
                    "SELECT EXISTS(SELECT 1 FROM track_user_tags WHERE tag = ?)",
                )
                .bind(&item.target)
                .fetch_one(&mut *transaction)
                .await
                .map_err(box_storage)?
                    != 0;
                sqlx::query(
                    "INSERT OR IGNORE INTO track_user_tags (track_id, tag, created_at) \
                     SELECT track_id, ?, CURRENT_TIMESTAMP FROM track_user_tags WHERE tag = ?",
                )
                .bind(&item.target)
                .bind(&item.source)
                .execute(&mut *transaction)
                .await
                .map_err(box_storage)?;
                sqlx::query("DELETE FROM track_user_tags WHERE tag = ?")
                    .bind(&item.source)
                    .execute(&mut *transaction)
                    .await
                    .map_err(box_storage)?;
                applied.push(RenameTagOutcome {
                    source: item.source.clone(),
                    target: item.target.clone(),
                    affected_tracks: usize::try_from(affected).map_err(|_| {
                        box_storage(StorageError::InvalidAssistantRecord("tag count overflow"))
                    })?,
                    merged,
                });
            }
            let next_usage = load_tag_usage(&mut transaction).await?;
            let next_signature = catalog_signature(&next_usage).map_err(|_| {
                box_storage(StorageError::InvalidAssistantRecord(
                    "invalid catalog signature",
                ))
            })?;
            transaction.commit().await.map_err(box_storage)?;
            Ok(CleanupMutation::Applied(CleanupApplyOutcome {
                applied,
                catalog_signature: next_signature,
            }))
        })
    }

    fn review_analysis<'a>(
        &'a self,
        targets: &'a [AnalysisReviewTarget],
        decision: AnalysisReviewDecision,
    ) -> AssistantFuture<'a, AnalysisReviewBatch> {
        Box::pin(async move {
            let _admission = self.write_gate.lock().await;
            let mut transaction = self.pool.begin().await.map_err(box_storage)?;
            let mut valid = Vec::<AnalysisReviewTarget>::new();
            let mut failures = Vec::new();
            for target in targets {
                let Some(track) = load_track(&mut transaction, target.track_id).await? else {
                    failures.push(review_failure(
                        target,
                        AnalysisReviewFailureCode::NotFound,
                        "Track not found",
                    ));
                    continue;
                };
                if !matches!(
                    target.analyzer_id.as_str(),
                    LOCAL_METADATA_ANALYZER_ID | CATALOG_TAG_ANALYZER_ID
                ) {
                    failures.push(review_failure(
                        target,
                        AnalysisReviewFailureCode::NotFound,
                        "Analysis profile not found",
                    ));
                    continue;
                }
                let row = sqlx::query(
                    "SELECT source_signature, moods_json FROM track_analyses \
                     WHERE track_id = ? AND analyzer_id = ?",
                )
                .bind(target.track_id.get())
                .bind(&target.analyzer_id)
                .fetch_optional(&mut *transaction)
                .await
                .map_err(box_storage)?;
                let Some(row) = row else {
                    failures.push(review_failure(
                        target,
                        AnalysisReviewFailureCode::NotFound,
                        "Analysis profile not found",
                    ));
                    continue;
                };
                let stored_signature: String =
                    row.try_get("source_signature").map_err(box_storage)?;
                if stored_signature != target.source_signature {
                    failures.push(review_failure(
                        target,
                        AnalysisReviewFailureCode::Stale,
                        "Analysis changed; refresh before reviewing this tag",
                    ));
                    continue;
                }
                let current_signature = metadata_source_signature(&track).map_err(|_| {
                    box_storage(StorageError::InvalidAssistantRecord(
                        "invalid source signature",
                    ))
                })?;
                if current_signature != target.source_signature {
                    failures.push(review_failure(target, AnalysisReviewFailureCode::Stale, "Track or analyzer settings changed; rerun analysis before reviewing this tag"));
                    continue;
                }
                let moods: Vec<String> = serde_json::from_str(
                    row.try_get::<&str, _>("moods_json").map_err(box_storage)?,
                )
                .map_err(StorageError::AssistantSerialization)
                .map_err(box_storage)?;
                if !moods.iter().any(|tag| tag == &target.tag) {
                    failures.push(review_failure(
                        target,
                        AnalysisReviewFailureCode::NotFound,
                        "Tag is not present in the current analysis profile",
                    ));
                    continue;
                }
                valid.push(target.clone());
            }
            let track_ids = valid
                .iter()
                .map(|target| target.track_id.get())
                .collect::<BTreeSet<_>>();
            let mut manual = load_manual_tags(&mut transaction, &track_ids).await?;
            let mut additions = BTreeMap::<i64, BTreeSet<String>>::new();
            if decision == AnalysisReviewDecision::Accepted {
                for target in &valid {
                    if !manual
                        .entry(target.track_id.get())
                        .or_default()
                        .contains(&target.tag)
                    {
                        additions
                            .entry(target.track_id.get())
                            .or_default()
                            .insert(target.tag.clone());
                    }
                }
            }
            let overflow = additions
                .iter()
                .filter(|(track_id, tags)| {
                    manual.get(track_id).map_or(0, BTreeSet::len) + tags.len() > MAX_TAGS_PER_TRACK
                })
                .map(|(track_id, _)| *track_id)
                .collect::<BTreeSet<_>>();
            let mut applied = Vec::new();
            for target in valid {
                let current_tags = manual.entry(target.track_id.get()).or_default();
                if decision == AnalysisReviewDecision::Accepted
                    && overflow.contains(&target.track_id.get())
                    && !current_tags.contains(&target.tag)
                {
                    failures.push(review_failure(
                        &target,
                        AnalysisReviewFailureCode::TagLimit,
                        &format!(
                            "selected suggestions would exceed the {MAX_TAGS_PER_TRACK}-tag limit"
                        ),
                    ));
                    continue;
                }
                match decision {
                    AnalysisReviewDecision::Pending => {
                        sqlx::query(
                            "DELETE FROM track_analysis_tag_reviews \
                             WHERE track_id = ? AND analyzer_id = ? AND tag = ?",
                        )
                        .bind(target.track_id.get())
                        .bind(&target.analyzer_id)
                        .bind(&target.tag)
                        .execute(&mut *transaction)
                        .await
                        .map_err(box_storage)?;
                    }
                    AnalysisReviewDecision::Accepted | AnalysisReviewDecision::Rejected => {
                        if decision == AnalysisReviewDecision::Accepted
                            && current_tags.insert(target.tag.clone())
                        {
                            sqlx::query(
                                "INSERT INTO track_user_tags (track_id, tag, created_at) \
                                 VALUES (?, ?, CURRENT_TIMESTAMP)",
                            )
                            .bind(target.track_id.get())
                            .bind(&target.tag)
                            .execute(&mut *transaction)
                            .await
                            .map_err(box_storage)?;
                        }
                        sqlx::query(
                            "INSERT INTO track_analysis_tag_reviews \
                             (track_id, analyzer_id, tag, source_signature, decision, reviewed_at) \
                             VALUES (?, ?, ?, ?, ?, CURRENT_TIMESTAMP) \
                             ON CONFLICT(track_id, analyzer_id, tag) DO UPDATE SET \
                               source_signature = excluded.source_signature, \
                               decision = excluded.decision, reviewed_at = excluded.reviewed_at",
                        )
                        .bind(target.track_id.get())
                        .bind(&target.analyzer_id)
                        .bind(&target.tag)
                        .bind(&target.source_signature)
                        .bind(decision.as_str())
                        .execute(&mut *transaction)
                        .await
                        .map_err(box_storage)?;
                    }
                }
                applied.push(AnalysisReviewOutcome {
                    track_id: target.track_id,
                    tag: target.tag,
                    analyzer_id: target.analyzer_id,
                    source_signature: target.source_signature,
                    decision,
                });
            }
            transaction.commit().await.map_err(box_storage)?;
            Ok(AnalysisReviewBatch {
                requested_items: targets.len(),
                applied,
                failures,
            })
        })
    }
}

fn analysis_from_row(row: &sqlx::sqlite::SqliteRow) -> Option<StoredAnalysis> {
    let moods = serde_json::from_str::<Vec<String>>(row.try_get("moods_json").ok()?).ok()?;
    let evidence = serde_json::from_str::<Vec<String>>(row.try_get("evidence_json").ok()?).ok()?;
    let metrics =
        serde_json::from_str::<Map<String, Value>>(row.try_get("metrics_json").ok()?).ok()?;
    Some(StoredAnalysis {
        analyzer_id: row.try_get("analyzer_id").ok()?,
        source_signature: row.try_get("source_signature").ok()?,
        energy: row.try_get("energy").ok()?,
        brightness: row.try_get("brightness").ok()?,
        tension: row.try_get("tension").ok()?,
        moods,
        evidence,
        metrics,
        confidence: row.try_get("confidence").ok()?,
    })
}

async fn load_vocabulary(
    pool: &sqlx::SqlitePool,
) -> Result<Option<TagVocabularyRecord>, StorageError> {
    let row = sqlx::query(
        "SELECT revision, seed_version, document_json FROM assistant_tag_vocabularies WHERE \"key\" = ?",
    )
    .bind(VOCABULARY_KEY)
    .fetch_optional(pool)
    .await?;
    row.as_ref().map(vocabulary_from_row).transpose()
}

async fn load_vocabulary_tx(
    transaction: &mut Transaction<'_, Sqlite>,
) -> Result<Option<TagVocabularyRecord>, AssistantDependencyErrorAlias> {
    let row = sqlx::query(
        "SELECT revision, seed_version, document_json FROM assistant_tag_vocabularies WHERE \"key\" = ?",
    )
    .bind(VOCABULARY_KEY)
    .fetch_optional(&mut **transaction)
    .await
    .map_err(box_storage)?;
    row.as_ref()
        .map(vocabulary_from_row)
        .transpose()
        .map_err(box_storage)
}

type AssistantDependencyErrorAlias = Box<dyn std::error::Error + Send + Sync>;

fn vocabulary_from_row(row: &sqlx::sqlite::SqliteRow) -> Result<TagVocabularyRecord, StorageError> {
    let revision = u32::try_from(row.try_get::<i64, _>("revision")?)
        .map_err(|_| StorageError::InvalidAssistantRecord("invalid vocabulary revision"))?;
    let seed_version = u32::try_from(row.try_get::<i64, _>("seed_version")?)
        .map_err(|_| StorageError::InvalidAssistantRecord("invalid vocabulary seed version"))?;
    let document = serde_json::from_str(row.try_get("document_json")?)
        .map_err(StorageError::AssistantSerialization)?;
    Ok(TagVocabularyRecord {
        revision,
        seed_version,
        document,
    })
}

async fn existing_track_ids(
    transaction: &mut Transaction<'_, Sqlite>,
    track_ids: &[i64],
) -> Result<BTreeSet<i64>, AssistantDependencyErrorAlias> {
    if track_ids.is_empty() {
        return Ok(BTreeSet::new());
    }
    let mut query = QueryBuilder::<Sqlite>::new("SELECT id FROM tracks WHERE id IN (");
    let mut separated = query.separated(",");
    for track_id in track_ids {
        separated.push_bind(track_id);
    }
    query.push(")");
    let values = query
        .build_query_scalar::<i64>()
        .fetch_all(&mut **transaction)
        .await
        .map_err(box_storage)?;
    Ok(values.into_iter().collect())
}

async fn load_manual_tags(
    transaction: &mut Transaction<'_, Sqlite>,
    track_ids: &BTreeSet<i64>,
) -> Result<BTreeMap<i64, BTreeSet<String>>, AssistantDependencyErrorAlias> {
    if track_ids.is_empty() {
        return Ok(BTreeMap::new());
    }
    let mut query = QueryBuilder::<Sqlite>::new(
        "SELECT track_id, tag FROM track_user_tags WHERE track_id IN (",
    );
    let mut separated = query.separated(",");
    for track_id in track_ids {
        separated.push_bind(track_id);
    }
    query.push(") ORDER BY track_id, tag");
    let rows = query
        .build()
        .fetch_all(&mut **transaction)
        .await
        .map_err(box_storage)?;
    let mut result = BTreeMap::<i64, BTreeSet<String>>::new();
    for row in rows {
        result
            .entry(row.try_get("track_id").map_err(box_storage)?)
            .or_default()
            .insert(row.try_get("tag").map_err(box_storage)?);
    }
    Ok(result)
}

async fn load_tag_usage(
    transaction: &mut Transaction<'_, Sqlite>,
) -> Result<Vec<TagUsage>, AssistantDependencyErrorAlias> {
    let rows = sqlx::query(
        "SELECT tag, COUNT(track_id) AS track_count FROM track_user_tags \
         GROUP BY tag ORDER BY tag",
    )
    .fetch_all(&mut **transaction)
    .await
    .map_err(box_storage)?;
    rows.iter()
        .map(|row| {
            Ok(TagUsage {
                tag: row.try_get("tag").map_err(box_storage)?,
                track_count: u64::try_from(
                    row.try_get::<i64, _>("track_count").map_err(box_storage)?,
                )
                .map_err(|_| {
                    box_storage(StorageError::InvalidAssistantRecord("invalid tag usage"))
                })?,
            })
        })
        .collect()
}

async fn load_track(
    transaction: &mut Transaction<'_, Sqlite>,
    track_id: TrackId,
) -> Result<Option<music_domain::IndexedTrack>, AssistantDependencyErrorAlias> {
    let query = format!("SELECT {TRACK_COLUMNS} FROM tracks WHERE id = ?");
    let row = sqlx::query(AssertSqlSafe(query))
        .bind(track_id.get())
        .fetch_optional(&mut **transaction)
        .await
        .map_err(box_storage)?;
    row.as_ref()
        .map(indexed_track_from_row)
        .transpose()
        .map_err(box_storage)
}

fn review_failure(
    target: &AnalysisReviewTarget,
    code: AnalysisReviewFailureCode,
    error: &str,
) -> AnalysisReviewFailure {
    AnalysisReviewFailure {
        target: target.clone(),
        code,
        error: error.to_owned(),
    }
}

fn box_storage(source: impl Into<StorageError>) -> AssistantDependencyErrorAlias {
    Box::new(source.into())
}

#[cfg(test)]
mod tests {
    use std::error::Error;

    use music_application::assistant::{AssistantService, CleanupSelection};
    use tempfile::TempDir;

    use super::*;
    use crate::SqliteStorageOptions;

    async fn storage() -> Result<(TempDir, SqliteStorage), Box<dyn Error + Send + Sync>> {
        let directory = tempfile::tempdir()?;
        let storage =
            SqliteStorage::open(SqliteStorageOptions::new(directory.path().join("music.db")))
                .await?;
        sqlx::query("INSERT INTO tracks (path, title, artist, album_artist, album, track_no, disc_no, year, genre, length_s, bpm, display_title, origin, size_bytes, mtime, added_at) VALUES ('a.mp3', 'A', '', '', '', NULL, NULL, NULL, '', 60.0, NULL, 'A', '', 1, 1, CURRENT_TIMESTAMP), ('b.mp3', 'B', '', '', '', NULL, NULL, NULL, '', 60.0, NULL, 'B', '', 1, 1, CURRENT_TIMESTAMP)")
            .execute(&storage.pool).await?;
        Ok((directory, storage))
    }

    #[tokio::test]
    async fn vocabulary_initialization_and_compare_swap_are_atomic()
    -> Result<(), Box<dyn Error + Send + Sync>> {
        let (_directory, storage) = storage().await?;
        let service = AssistantService::new(std::sync::Arc::new(storage));
        let first = service.vocabulary().await?;
        let updated = service
            .replace_vocabulary(first.revision, first.document.clone())
            .await?;
        assert_eq!(updated.revision, first.revision + 1);
        assert!(
            service
                .replace_vocabulary(first.revision, first.document)
                .await
                .is_err()
        );
        Ok(())
    }

    #[tokio::test]
    async fn cleanup_apply_rejects_stale_catalogs_and_merges_atomically()
    -> Result<(), Box<dyn Error + Send + Sync>> {
        let (_directory, storage) = storage().await?;
        sqlx::query("INSERT INTO track_user_tags (track_id, tag, created_at) VALUES (1, 'inn', CURRENT_TIMESTAMP), (2, 'tavern', CURRENT_TIMESTAMP)")
            .execute(&storage.pool).await?;
        let service = AssistantService::new(std::sync::Arc::new(storage));
        let preview = service.cleanup_preview().await?;
        let applied = service
            .apply_cleanup(
                &preview.catalog_signature,
                &preview.vocabulary_fingerprint,
                &[CleanupSelection {
                    source: "inn".to_owned(),
                    target: "tavern".to_owned(),
                }],
            )
            .await?;
        assert_eq!(applied.applied[0].affected_tracks, 1);
        assert!(applied.applied[0].merged);
        assert!(
            service
                .apply_cleanup(
                    &preview.catalog_signature,
                    &preview.vocabulary_fingerprint,
                    &[CleanupSelection {
                        source: "inn".to_owned(),
                        target: "tavern".to_owned()
                    }],
                )
                .await
                .is_err()
        );
        Ok(())
    }

    #[tokio::test]
    async fn reviewed_cleanup_accepts_only_pairs_from_the_immutable_model_proposal()
    -> Result<(), Box<dyn Error + Send + Sync>> {
        let (_directory, storage) = storage().await?;
        sqlx::query(
            "INSERT INTO track_user_tags (track_id, tag, created_at) VALUES (1, 'clue hunting', CURRENT_TIMESTAMP)",
        )
        .execute(&storage.pool)
        .await?;
        let service = AssistantService::new(std::sync::Arc::new(storage));
        let preview = service.cleanup_preview().await?;
        let allowed = [CleanupSelection {
            source: "clue hunting".to_owned(),
            target: "investigation".to_owned(),
        }];
        assert!(
            service
                .apply_reviewed_cleanup(
                    &preview.catalog_signature,
                    &preview.vocabulary_fingerprint,
                    &[CleanupSelection {
                        source: "clue hunting".to_owned(),
                        target: "calm".to_owned(),
                    }],
                    &allowed,
                )
                .await
                .is_err()
        );
        let applied = service
            .apply_reviewed_cleanup(
                &preview.catalog_signature,
                &preview.vocabulary_fingerprint,
                &allowed,
                &allowed,
            )
            .await?;
        assert_eq!(applied.applied.len(), 1);
        assert_eq!(applied.applied[0].target, "investigation");
        Ok(())
    }
}
