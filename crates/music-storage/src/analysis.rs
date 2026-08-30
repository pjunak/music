use music_application::assistant::{
    AnalysisFailureState, AnalysisFailureWrite, AnalysisState, AnalysisWrite, AssistantFuture,
    ContextState, ContextWrite, LOCAL_AUDIO_ANALYZER_ID, LOCAL_CONTEXT_ANALYZER_ID,
    LOCAL_CONTEXT_IMPLEMENTATION_ID, LOCAL_METADATA_ANALYZER_ID, LocalAnalysisRepository,
    MAX_MODEL_EVIDENCE_ITEMS, MAX_MODEL_TAGS_PER_TRACK, MODEL_TAG_ANALYZER_ID, ModelAnalysisWrite,
    audio_source_signature, context_source_signature, metadata_source_signature,
    model_tag_source_signature, parse_context_state,
};
use sqlx::sqlite::SqliteRow;
use sqlx::{AssertSqlSafe, Row, Sqlite, Transaction};

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

    fn analysis_failures<'a>(
        &'a self,
        analyzer_id: &'a str,
    ) -> AssistantFuture<'a, Vec<AnalysisFailureState>> {
        Box::pin(async move {
            let rows = sqlx::query(
                "SELECT track_id, source_signature, job_id, error, \
                 CAST(strftime('%s', updated_at) AS INTEGER) AS updated_at_unix_seconds \
                 FROM track_analysis_failures WHERE analyzer_id = ? ORDER BY track_id",
            )
            .bind(analyzer_id)
            .fetch_all(&self.pool)
            .await
            .map_err(box_storage)?;
            rows.iter()
                .map(|row| {
                    let raw_id: i64 = row.try_get("track_id").map_err(StorageError::from)?;
                    Ok(AnalysisFailureState {
                        track_id: music_domain::TrackId::new(raw_id).map_err(|_| {
                            StorageError::InvalidAssistantRecord(
                                "analysis failure track id is invalid",
                            )
                        })?,
                        source_signature: row
                            .try_get("source_signature")
                            .map_err(StorageError::from)?,
                        job_id: row.try_get("job_id").map_err(StorageError::from)?,
                        error: row.try_get("error").map_err(StorageError::from)?,
                        updated_at_unix_seconds: row
                            .try_get::<Option<i64>, _>("updated_at_unix_seconds")
                            .map_err(StorageError::from)?
                            .ok_or(StorageError::InvalidAssistantRecord(
                                "analysis failure timestamp is invalid",
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

    fn store_audio_analysis<'a>(
        &'a self,
        analyzer_id: &'a str,
        job_id: &'a str,
        profile: &'a AnalysisWrite,
    ) -> AssistantFuture<'a, bool> {
        Box::pin(async move {
            if analyzer_id != LOCAL_AUDIO_ANALYZER_ID || !valid_audio_profile(profile) {
                return Err(box_storage(StorageError::InvalidAssistantRecord(
                    "audio analysis profile is invalid",
                )));
            }
            let _admission = self.write_gate.lock().await;
            let mut transaction = self.pool.begin().await.map_err(box_storage)?;
            let Some(track) = load_track(&mut transaction, profile.track_id).await? else {
                transaction.commit().await.map_err(box_storage)?;
                return Ok(false);
            };
            let current_signature = audio_source_signature(&track).map_err(|_| {
                box_storage(StorageError::InvalidAssistantRecord(
                    "track audio fingerprint is invalid",
                ))
            })?;
            if current_signature != profile.source_signature {
                transaction.commit().await.map_err(box_storage)?;
                return Ok(false);
            }
            let evidence = serde_json::to_string(&profile.evidence)
                .map_err(StorageError::AssistantSerialization)
                .map_err(box_storage)?;
            let metrics = serde_json::to_string(&profile.metrics)
                .map_err(StorageError::AssistantSerialization)
                .map_err(box_storage)?;
            sqlx::query(
                "INSERT INTO track_analyses \
                 (track_id, analyzer_id, source_signature, job_id, energy, brightness, tension, \
                  moods_json, evidence_json, metrics_json, confidence, updated_at) \
                 VALUES (?, ?, ?, ?, ?, ?, ?, '[]', ?, ?, ?, CURRENT_TIMESTAMP) \
                 ON CONFLICT(track_id, analyzer_id) DO UPDATE SET \
                   source_signature = excluded.source_signature, job_id = excluded.job_id, \
                   energy = excluded.energy, brightness = excluded.brightness, \
                   tension = excluded.tension, moods_json = '[]', \
                   evidence_json = excluded.evidence_json, metrics_json = excluded.metrics_json, \
                   confidence = excluded.confidence, updated_at = CURRENT_TIMESTAMP",
            )
            .bind(profile.track_id.get())
            .bind(analyzer_id)
            .bind(&profile.source_signature)
            .bind(job_id)
            .bind(profile.energy)
            .bind(profile.brightness)
            .bind(profile.tension)
            .bind(evidence)
            .bind(metrics)
            .bind(profile.confidence.as_str())
            .execute(&mut *transaction)
            .await
            .map_err(box_storage)?;
            sqlx::query(
                "DELETE FROM track_analysis_failures WHERE track_id = ? AND analyzer_id = ?",
            )
            .bind(profile.track_id.get())
            .bind(analyzer_id)
            .execute(&mut *transaction)
            .await
            .map_err(box_storage)?;
            transaction.commit().await.map_err(box_storage)?;
            Ok(true)
        })
    }

    fn store_model_analysis<'a>(
        &'a self,
        analyzer_id: &'a str,
        job_id: &'a str,
        role_fingerprint: &'a str,
        vocabulary_fingerprint: &'a str,
        voice_signature: Option<&'a str>,
        profiles: &'a [ModelAnalysisWrite],
    ) -> AssistantFuture<'a, usize> {
        Box::pin(async move {
            if analyzer_id != MODEL_TAG_ANALYZER_ID
                || !valid_hex_digest(role_fingerprint)
                || !valid_hex_digest(vocabulary_fingerprint)
            {
                return Err(box_storage(StorageError::InvalidAssistantRecord(
                    "model analysis contract is invalid",
                )));
            }
            let _admission = self.write_gate.lock().await;
            let mut transaction = self.pool.begin().await.map_err(box_storage)?;
            let mut stored = 0_usize;
            for document in profiles {
                let profile = &document.profile;
                if !valid_model_profile(profile) {
                    return Err(box_storage(StorageError::InvalidAssistantRecord(
                        "model analysis profile is invalid",
                    )));
                }
                let Some(track) = load_track(&mut transaction, profile.track_id).await? else {
                    continue;
                };
                let expected_context_signature = context_source_signature(
                    &track,
                    LOCAL_CONTEXT_IMPLEMENTATION_ID,
                    voice_signature,
                )
                .map_err(|_| {
                    box_storage(StorageError::InvalidAssistantRecord(
                        "track context fingerprint is invalid",
                    ))
                })?;
                let context_row = sqlx::query(
                    "SELECT source_signature, job_id, completeness, confidence, summary_json, \
                     timeline_json, sections_json, technical_json, stages_json, \
                     CAST(strftime('%s', updated_at) AS INTEGER) AS updated_at_unix_seconds \
                     FROM track_contexts WHERE track_id = ? AND analyzer_id = ?",
                )
                .bind(track.id.get())
                .bind(LOCAL_CONTEXT_ANALYZER_ID)
                .fetch_optional(&mut *transaction)
                .await
                .map_err(box_storage)?;
                let current_context = context_row
                    .as_ref()
                    .filter(|row| {
                        row.try_get::<String, _>("source_signature")
                            .is_ok_and(|signature| signature == expected_context_signature)
                    })
                    .and_then(|row| context_state_from_row(track.id, row).ok())
                    .and_then(|state| parse_context_state(&state));
                let current_signature = model_tag_source_signature(
                    &track,
                    role_fingerprint,
                    vocabulary_fingerprint,
                    current_context.as_ref(),
                )
                .map_err(|_| {
                    box_storage(StorageError::InvalidAssistantRecord(
                        "model tag fingerprint is invalid",
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
                let metrics = serde_json::to_string(&profile.metrics)
                    .map_err(StorageError::AssistantSerialization)
                    .map_err(box_storage)?;
                sqlx::query(
                    "INSERT INTO track_analyses \
                     (track_id, analyzer_id, source_signature, job_id, energy, brightness, \
                      tension, moods_json, evidence_json, metrics_json, confidence, updated_at) \
                     VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, CURRENT_TIMESTAMP) \
                     ON CONFLICT(track_id, analyzer_id) DO UPDATE SET \
                       source_signature = excluded.source_signature, job_id = excluded.job_id, \
                       energy = excluded.energy, brightness = excluded.brightness, \
                       tension = excluded.tension, moods_json = excluded.moods_json, \
                       evidence_json = excluded.evidence_json, metrics_json = excluded.metrics_json, \
                       confidence = excluded.confidence, updated_at = CURRENT_TIMESTAMP",
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
                .bind(metrics)
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

    fn store_analysis_failure<'a>(
        &'a self,
        analyzer_id: &'a str,
        job_id: &'a str,
        failure: &'a AnalysisFailureWrite,
    ) -> AssistantFuture<'a, bool> {
        Box::pin(async move {
            if analyzer_id != LOCAL_AUDIO_ANALYZER_ID
                || failure.source_signature.len() != 64
                || failure.error.is_empty()
            {
                return Err(box_storage(StorageError::InvalidAssistantRecord(
                    "audio analysis failure is invalid",
                )));
            }
            let _admission = self.write_gate.lock().await;
            let mut transaction = self.pool.begin().await.map_err(box_storage)?;
            let Some(track) = load_track(&mut transaction, failure.track_id).await? else {
                transaction.commit().await.map_err(box_storage)?;
                return Ok(false);
            };
            let current_signature = audio_source_signature(&track).map_err(|_| {
                box_storage(StorageError::InvalidAssistantRecord(
                    "track audio fingerprint is invalid",
                ))
            })?;
            if current_signature != failure.source_signature {
                transaction.commit().await.map_err(box_storage)?;
                return Ok(false);
            }
            let error = truncate_utf8(&failure.error, 2_000);
            sqlx::query(
                "INSERT INTO track_analysis_failures \
                 (track_id, analyzer_id, source_signature, job_id, error, updated_at) \
                 VALUES (?, ?, ?, ?, ?, CURRENT_TIMESTAMP) \
                 ON CONFLICT(track_id, analyzer_id) DO UPDATE SET \
                   source_signature = excluded.source_signature, job_id = excluded.job_id, \
                   error = excluded.error, updated_at = CURRENT_TIMESTAMP",
            )
            .bind(failure.track_id.get())
            .bind(analyzer_id)
            .bind(&failure.source_signature)
            .bind(job_id)
            .bind(error)
            .execute(&mut *transaction)
            .await
            .map_err(box_storage)?;
            transaction.commit().await.map_err(box_storage)?;
            Ok(true)
        })
    }

    fn context_states<'a>(
        &'a self,
        analyzer_id: &'a str,
    ) -> AssistantFuture<'a, Vec<ContextState>> {
        Box::pin(async move {
            if analyzer_id != LOCAL_CONTEXT_ANALYZER_ID {
                return Err(box_storage(StorageError::InvalidAssistantRecord(
                    "context analyzer id is invalid",
                )));
            }
            let rows = sqlx::query(
                "SELECT track_id, source_signature, job_id, completeness, confidence, \
                 summary_json, timeline_json, sections_json, technical_json, stages_json, \
                 CAST(strftime('%s', updated_at) AS INTEGER) AS updated_at_unix_seconds \
                 FROM track_contexts WHERE analyzer_id = ? ORDER BY track_id",
            )
            .bind(analyzer_id)
            .fetch_all(&self.pool)
            .await
            .map_err(box_storage)?;
            rows.iter()
                .map(|row| {
                    let raw_id: i64 = row.try_get("track_id").map_err(StorageError::from)?;
                    Ok(ContextState {
                        track_id: music_domain::TrackId::new(raw_id).map_err(|_| {
                            StorageError::InvalidAssistantRecord("context track id is invalid")
                        })?,
                        source_signature: row
                            .try_get("source_signature")
                            .map_err(StorageError::from)?,
                        job_id: row.try_get("job_id").map_err(StorageError::from)?,
                        completeness: row.try_get("completeness").map_err(StorageError::from)?,
                        confidence: row.try_get("confidence").map_err(StorageError::from)?,
                        summary_json: row.try_get("summary_json").map_err(StorageError::from)?,
                        timeline_json: row.try_get("timeline_json").map_err(StorageError::from)?,
                        sections_json: row.try_get("sections_json").map_err(StorageError::from)?,
                        technical_json: row
                            .try_get("technical_json")
                            .map_err(StorageError::from)?,
                        stages_json: row.try_get("stages_json").map_err(StorageError::from)?,
                        updated_at_unix_seconds: row
                            .try_get::<Option<i64>, _>("updated_at_unix_seconds")
                            .map_err(StorageError::from)?
                            .ok_or(StorageError::InvalidAssistantRecord(
                                "context timestamp is invalid",
                            ))?,
                    })
                })
                .collect::<Result<Vec<_>, StorageError>>()
                .map_err(box_storage)
        })
    }

    fn store_context<'a>(
        &'a self,
        analyzer_id: &'a str,
        implementation_id: &'a str,
        voice_signature: Option<&'a str>,
        job_id: &'a str,
        document: &'a ContextWrite,
    ) -> AssistantFuture<'a, bool> {
        Box::pin(async move {
            if analyzer_id != LOCAL_CONTEXT_ANALYZER_ID
                || implementation_id != LOCAL_CONTEXT_IMPLEMENTATION_ID
                || !valid_context(document)
            {
                return Err(box_storage(StorageError::InvalidAssistantRecord(
                    "track context document is invalid",
                )));
            }
            let _admission = self.write_gate.lock().await;
            let mut transaction = self.pool.begin().await.map_err(box_storage)?;
            let Some(track) = load_track(&mut transaction, document.track_id).await? else {
                transaction.commit().await.map_err(box_storage)?;
                return Ok(false);
            };
            let current_signature =
                context_source_signature(&track, implementation_id, voice_signature).map_err(
                    |_| {
                        box_storage(StorageError::InvalidAssistantRecord(
                            "track context fingerprint is invalid",
                        ))
                    },
                )?;
            if current_signature != document.source_signature {
                transaction.commit().await.map_err(box_storage)?;
                return Ok(false);
            }
            let summary = context_json(&document.summary)?;
            let timeline = context_json(&document.timeline)?;
            let sections = context_json(&document.sections)?;
            let technical = context_json(&document.technical)?;
            let stages = context_json(&document.stages)?;
            sqlx::query(
                "INSERT INTO track_contexts \
                 (track_id, analyzer_id, source_signature, job_id, completeness, confidence, \
                  summary_json, timeline_json, sections_json, technical_json, stages_json, \
                  updated_at) \
                 VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, CURRENT_TIMESTAMP) \
                 ON CONFLICT(track_id, analyzer_id) DO UPDATE SET \
                   source_signature = excluded.source_signature, job_id = excluded.job_id, \
                   completeness = excluded.completeness, confidence = excluded.confidence, \
                   summary_json = excluded.summary_json, timeline_json = excluded.timeline_json, \
                   sections_json = excluded.sections_json, technical_json = excluded.technical_json, \
                   stages_json = excluded.stages_json, updated_at = CURRENT_TIMESTAMP",
            )
            .bind(document.track_id.get())
            .bind(analyzer_id)
            .bind(&document.source_signature)
            .bind(job_id)
            .bind(&document.completeness)
            .bind(&document.confidence)
            .bind(summary)
            .bind(timeline)
            .bind(sections)
            .bind(technical)
            .bind(stages)
            .execute(&mut *transaction)
            .await
            .map_err(box_storage)?;
            sqlx::query(
                "DELETE FROM track_analysis_failures WHERE track_id = ? AND analyzer_id = ?",
            )
            .bind(document.track_id.get())
            .bind(analyzer_id)
            .execute(&mut *transaction)
            .await
            .map_err(box_storage)?;
            transaction.commit().await.map_err(box_storage)?;
            Ok(true)
        })
    }

    fn store_context_failure<'a>(
        &'a self,
        analyzer_id: &'a str,
        implementation_id: &'a str,
        voice_signature: Option<&'a str>,
        job_id: &'a str,
        failure: &'a AnalysisFailureWrite,
    ) -> AssistantFuture<'a, bool> {
        Box::pin(async move {
            if analyzer_id != LOCAL_CONTEXT_ANALYZER_ID
                || implementation_id != LOCAL_CONTEXT_IMPLEMENTATION_ID
                || failure.source_signature.len() != 64
                || failure.error.is_empty()
            {
                return Err(box_storage(StorageError::InvalidAssistantRecord(
                    "track context failure is invalid",
                )));
            }
            let _admission = self.write_gate.lock().await;
            let mut transaction = self.pool.begin().await.map_err(box_storage)?;
            let Some(track) = load_track(&mut transaction, failure.track_id).await? else {
                transaction.commit().await.map_err(box_storage)?;
                return Ok(false);
            };
            let current_signature =
                context_source_signature(&track, implementation_id, voice_signature).map_err(
                    |_| {
                        box_storage(StorageError::InvalidAssistantRecord(
                            "track context fingerprint is invalid",
                        ))
                    },
                )?;
            if current_signature != failure.source_signature {
                transaction.commit().await.map_err(box_storage)?;
                return Ok(false);
            }
            sqlx::query(
                "INSERT INTO track_analysis_failures \
                 (track_id, analyzer_id, source_signature, job_id, error, updated_at) \
                 VALUES (?, ?, ?, ?, ?, CURRENT_TIMESTAMP) \
                 ON CONFLICT(track_id, analyzer_id) DO UPDATE SET \
                   source_signature = excluded.source_signature, job_id = excluded.job_id, \
                   error = excluded.error, updated_at = CURRENT_TIMESTAMP",
            )
            .bind(failure.track_id.get())
            .bind(analyzer_id)
            .bind(&failure.source_signature)
            .bind(job_id)
            .bind(truncate_utf8(&failure.error, 2_000))
            .execute(&mut *transaction)
            .await
            .map_err(box_storage)?;
            transaction.commit().await.map_err(box_storage)?;
            Ok(true)
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

fn valid_model_profile(profile: &AnalysisWrite) -> bool {
    [profile.energy, profile.brightness, profile.tension]
        .into_iter()
        .all(|value| value.is_finite() && (0.0..=1.0).contains(&value))
        && valid_hex_digest(&profile.source_signature)
        && profile.moods.len() <= MAX_MODEL_TAGS_PER_TRACK
        && profile.evidence.len() <= MAX_MODEL_EVIDENCE_ITEMS
        && profile
            .metrics
            .get("contract")
            .and_then(serde_json::Value::as_str)
            == Some("assistant-music-tagger-output/v3")
}

fn valid_hex_digest(value: &str) -> bool {
    value.len() == 64
        && value
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
}

fn context_state_from_row(
    track_id: music_domain::TrackId,
    row: &SqliteRow,
) -> Result<ContextState, StorageError> {
    Ok(ContextState {
        track_id,
        source_signature: row.try_get("source_signature")?,
        job_id: row.try_get("job_id")?,
        completeness: row.try_get("completeness")?,
        confidence: row.try_get("confidence")?,
        summary_json: row.try_get("summary_json")?,
        timeline_json: row.try_get("timeline_json")?,
        sections_json: row.try_get("sections_json")?,
        technical_json: row.try_get("technical_json")?,
        stages_json: row.try_get("stages_json")?,
        updated_at_unix_seconds: row
            .try_get::<Option<i64>, _>("updated_at_unix_seconds")?
            .ok_or(StorageError::InvalidAssistantRecord(
                "context timestamp is invalid",
            ))?,
    })
}

fn valid_audio_profile(profile: &AnalysisWrite) -> bool {
    valid_profile(profile)
        && profile.moods.is_empty()
        && profile
            .metrics
            .get("schema")
            .and_then(serde_json::Value::as_str)
            == Some(LOCAL_AUDIO_ANALYZER_ID)
        && profile.metrics.values().all(|value| {
            matches!(
                value,
                serde_json::Value::Null
                    | serde_json::Value::String(_)
                    | serde_json::Value::Number(_)
            )
        })
}

fn valid_context(document: &ContextWrite) -> bool {
    document.source_signature.len() == 64
        && matches!(document.completeness.as_str(), "full" | "partial")
        && matches!(document.confidence.as_str(), "high" | "medium" | "low")
        && document
            .summary
            .get("schema_version")
            .and_then(serde_json::Value::as_str)
            == Some(LOCAL_CONTEXT_ANALYZER_ID)
        && document.timeline.len() <= 43_200
        && document.sections.len() <= 10
        && document
            .timeline
            .iter()
            .flat_map(serde_json::Map::values)
            .all(serde_json::Value::is_number)
}

fn context_json(
    value: &impl serde::Serialize,
) -> Result<String, Box<dyn std::error::Error + Send + Sync>> {
    let encoded = serde_json::to_string(value)
        .map_err(StorageError::AssistantSerialization)
        .map_err(box_storage)?;
    if encoded.len() > 16 * 1_024 * 1_024 {
        return Err(box_storage(StorageError::InvalidAssistantRecord(
            "track context document is too large",
        )));
    }
    Ok(encoded)
}

async fn load_track(
    transaction: &mut Transaction<'_, Sqlite>,
    track_id: music_domain::TrackId,
) -> Result<Option<music_domain::IndexedTrack>, Box<dyn std::error::Error + Send + Sync>> {
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

fn truncate_utf8(value: &str, maximum_bytes: usize) -> &str {
    if value.len() <= maximum_bytes {
        return value;
    }
    let mut boundary = maximum_bytes;
    while boundary > 0 && !value.is_char_boundary(boundary) {
        boundary -= 1;
    }
    &value[..boundary]
}

fn box_storage(error: impl Into<StorageError>) -> Box<dyn std::error::Error + Send + Sync> {
    Box::new(error.into())
}

#[cfg(test)]
mod tests {
    use std::error::Error;

    use music_application::assistant::{
        AnalysisFailureWrite, AnalysisWrite, Confidence, ContextWrite, LOCAL_AUDIO_ANALYZER_ID,
        LOCAL_CONTEXT_ANALYZER_ID, LOCAL_CONTEXT_IMPLEMENTATION_ID, LOCAL_METADATA_ANALYZER_ID,
        LocalAnalysisRepository, MODEL_TAG_ANALYZER_ID, ModelAnalysisWrite, audio_source_signature,
        context_source_signature, metadata_source_signature, model_tag_source_signature,
        parse_context_state,
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
            metrics: serde_json::Map::new(),
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

    #[tokio::test]
    async fn audio_success_and_failure_check_identity_and_replace_failure_atomically()
    -> Result<(), Box<dyn Error + Send + Sync>> {
        let (_directory, storage) = storage().await?;
        let track = LibraryRepository::all_tracks(&storage).await?.remove(0);
        let signature = audio_source_signature(&track)?;
        let failure = AnalysisFailureWrite {
            track_id: track.id,
            source_signature: signature.clone(),
            error: "AudioSignalError: decoder failed".to_owned(),
        };
        assert!(
            LocalAnalysisRepository::store_analysis_failure(
                &storage,
                LOCAL_AUDIO_ANALYZER_ID,
                "audio-job",
                &failure,
            )
            .await?
        );
        assert_eq!(
            LocalAnalysisRepository::analysis_failures(&storage, LOCAL_AUDIO_ANALYZER_ID)
                .await?
                .len(),
            1
        );
        let profile = AnalysisWrite {
            track_id: track.id,
            source_signature: signature,
            energy: 0.4,
            brightness: 0.5,
            tension: 0.6,
            moods: Vec::new(),
            evidence: vec!["Signal level: test".to_owned()],
            metrics: serde_json::Map::from_iter([(
                "schema".to_owned(),
                serde_json::Value::String(LOCAL_AUDIO_ANALYZER_ID.to_owned()),
            )]),
            confidence: Confidence::Medium,
        };
        assert!(
            LocalAnalysisRepository::store_audio_analysis(
                &storage,
                LOCAL_AUDIO_ANALYZER_ID,
                "audio-job",
                &profile,
            )
            .await?
        );
        assert!(
            LocalAnalysisRepository::analysis_failures(&storage, LOCAL_AUDIO_ANALYZER_ID)
                .await?
                .is_empty()
        );
        assert_eq!(
            LocalAnalysisRepository::analysis_states(&storage, LOCAL_AUDIO_ANALYZER_ID).await?[0]
                .confidence,
            "medium"
        );
        sqlx::query("UPDATE tracks SET mtime = 21 WHERE id = ?")
            .bind(track.id.get())
            .execute(&storage.pool)
            .await?;
        assert!(
            !LocalAnalysisRepository::store_audio_analysis(
                &storage,
                LOCAL_AUDIO_ANALYZER_ID,
                "stale-job",
                &profile,
            )
            .await?
        );
        Ok(())
    }

    #[tokio::test]
    async fn context_success_rechecks_identity_and_replaces_failure_atomically()
    -> Result<(), Box<dyn Error + Send + Sync>> {
        let (_directory, storage) = storage().await?;
        let track = LibraryRepository::all_tracks(&storage).await?.remove(0);
        let signature = context_source_signature(&track, LOCAL_CONTEXT_IMPLEMENTATION_ID, None)?;
        let failure = AnalysisFailureWrite {
            track_id: track.id,
            source_signature: signature.clone(),
            error: "AudioContextError: decoder failed".to_owned(),
        };
        assert!(
            LocalAnalysisRepository::store_context_failure(
                &storage,
                LOCAL_CONTEXT_ANALYZER_ID,
                LOCAL_CONTEXT_IMPLEMENTATION_ID,
                None,
                "context-job",
                &failure,
            )
            .await?
        );
        let document = ContextWrite {
            track_id: track.id,
            source_signature: signature,
            completeness: "full".to_owned(),
            confidence: "medium".to_owned(),
            summary: serde_json::Map::from_iter([
                (
                    "schema_version".to_owned(),
                    serde_json::json!(LOCAL_CONTEXT_ANALYZER_ID),
                ),
                (
                    "voice".to_owned(),
                    serde_json::json!({"status": "not_classified"}),
                ),
            ]),
            timeline: vec![serde_json::Map::from_iter([
                ("start_s".to_owned(), serde_json::json!(0.0)),
                ("duration_s".to_owned(), serde_json::json!(1.0)),
            ])],
            sections: vec![serde_json::Map::from_iter([(
                "id".to_owned(),
                serde_json::json!("s1"),
            )])],
            technical: serde_json::Map::from_iter([(
                "probe_status".to_owned(),
                serde_json::json!("complete"),
            )]),
            stages: serde_json::Map::from_iter([(
                "voice".to_owned(),
                serde_json::json!({"status": "not_configured"}),
            )]),
        };
        assert!(
            LocalAnalysisRepository::store_context(
                &storage,
                LOCAL_CONTEXT_ANALYZER_ID,
                LOCAL_CONTEXT_IMPLEMENTATION_ID,
                None,
                "context-job",
                &document,
            )
            .await?
        );
        assert!(
            LocalAnalysisRepository::analysis_failures(&storage, LOCAL_CONTEXT_ANALYZER_ID)
                .await?
                .is_empty()
        );
        let states =
            LocalAnalysisRepository::context_states(&storage, LOCAL_CONTEXT_ANALYZER_ID).await?;
        assert_eq!(states.len(), 1);
        assert_eq!(states[0].completeness, "full");
        sqlx::query("UPDATE tracks SET mtime = 21 WHERE id = ?")
            .bind(track.id.get())
            .execute(&storage.pool)
            .await?;
        assert!(
            !LocalAnalysisRepository::store_context(
                &storage,
                LOCAL_CONTEXT_ANALYZER_ID,
                LOCAL_CONTEXT_IMPLEMENTATION_ID,
                None,
                "stale-job",
                &document,
            )
            .await?
        );
        Ok(())
    }

    #[tokio::test]
    async fn model_analysis_rechecks_metadata_context_role_and_vocabulary_in_one_transaction()
    -> Result<(), Box<dyn Error + Send + Sync>> {
        let (_directory, storage) = storage().await?;
        let track = LibraryRepository::all_tracks(&storage).await?.remove(0);
        let context_signature =
            context_source_signature(&track, LOCAL_CONTEXT_IMPLEMENTATION_ID, None)?;
        let context = ContextWrite {
            track_id: track.id,
            source_signature: context_signature,
            completeness: "full".to_owned(),
            confidence: "medium".to_owned(),
            summary: serde_json::Map::from_iter([(
                "schema_version".to_owned(),
                serde_json::json!(LOCAL_CONTEXT_ANALYZER_ID),
            )]),
            timeline: vec![serde_json::Map::from_iter([(
                "intensity".to_owned(),
                serde_json::json!(0.5),
            )])],
            sections: vec![serde_json::Map::from_iter([(
                "id".to_owned(),
                serde_json::json!("s1"),
            )])],
            technical: serde_json::Map::new(),
            stages: serde_json::Map::new(),
        };
        assert!(
            LocalAnalysisRepository::store_context(
                &storage,
                LOCAL_CONTEXT_ANALYZER_ID,
                LOCAL_CONTEXT_IMPLEMENTATION_ID,
                None,
                "context-job",
                &context,
            )
            .await?
        );
        let context_state =
            LocalAnalysisRepository::context_states(&storage, LOCAL_CONTEXT_ANALYZER_ID)
                .await?
                .remove(0);
        let current_context = parse_context_state(&context_state).ok_or("context did not parse")?;
        let role_fingerprint = "a".repeat(64);
        let vocabulary_fingerprint = "b".repeat(64);
        let source_signature = model_tag_source_signature(
            &track,
            &role_fingerprint,
            &vocabulary_fingerprint,
            Some(&current_context),
        )?;
        let document = ModelAnalysisWrite {
            profile: AnalysisWrite {
                track_id: track.id,
                source_signature,
                energy: 0.6,
                brightness: 0.5,
                tension: 0.7,
                moods: vec!["tense".to_owned()],
                evidence: vec!["context section s1".to_owned()],
                metrics: serde_json::json!({
                    "contract": "assistant-music-tagger-output/v3",
                    "input_contract": "assistant-music-tagger-input/v18",
                })
                .as_object()
                .cloned()
                .ok_or("metrics were not an object")?,
                confidence: Confidence::Medium,
            },
        };
        assert_eq!(
            LocalAnalysisRepository::store_model_analysis(
                &storage,
                MODEL_TAG_ANALYZER_ID,
                "model-job",
                &role_fingerprint,
                &vocabulary_fingerprint,
                None,
                std::slice::from_ref(&document),
            )
            .await?,
            1
        );
        sqlx::query(
            "UPDATE track_contexts SET source_signature = ? WHERE track_id = ? AND analyzer_id = ?",
        )
        .bind("c".repeat(64))
        .bind(track.id.get())
        .bind(LOCAL_CONTEXT_ANALYZER_ID)
        .execute(&storage.pool)
        .await?;
        assert_eq!(
            LocalAnalysisRepository::store_model_analysis(
                &storage,
                MODEL_TAG_ANALYZER_ID,
                "stale-model-job",
                &role_fingerprint,
                &vocabulary_fingerprint,
                None,
                &[document],
            )
            .await?,
            0
        );
        Ok(())
    }
}
