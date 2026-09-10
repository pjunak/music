use super::{CleanupEnrichmentRepository, cleanup_enrichment_source_signature};
use crate::assistant::{
    CleanupModelDecision, LIBRARY_CLEANUP_DISCLOSURE, LIBRARY_CLEANUP_ENGINE_ID,
    LIBRARY_CLEANUP_QUALITY_ID, LibraryCleanupModelTask, ModelQualityService,
    ModelReviewDestination, ModelRunManifest, ProviderUsageAccumulator, StructuredModelTransport,
    execute_recorded_provider_request,
};
use crate::cleanup::{CleanupScope, CleanupService};
use crate::cleanup_sources::CleanupSourceService;
use crate::jobs::{
    JobCheckpointPolicy, JobDefinition, JobExecutionContext, JobHandler, JobHandlerError,
    JobHandlerFuture, JobLane, JobStatus,
};
use music_domain::TrackId;
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value, json};
use std::sync::Arc;

pub const CLEANUP_AI_JOB_KIND: &str = "assistant.model-library-cleanup";

#[derive(Debug)]
pub struct CleanupAiJobHandler {
    pub cleanup: Arc<CleanupService>,
    pub cache: Arc<dyn CleanupEnrichmentRepository>,
    pub sources: Arc<CleanupSourceService>,
    pub quality: Arc<ModelQualityService>,
    pub transport: Arc<dyn StructuredModelTransport>,
}

#[derive(Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CleanupAiParameters {
    pub track_id: i64,
    pub catalog_job_id: String,
    pub consent: bool,
    pub disclosure_version: String,
    pub role_fingerprint: String,
}

impl CleanupAiJobHandler {
    async fn run(
        &self,
        context: &JobExecutionContext,
        parameters: CleanupAiParameters,
    ) -> Result<Value, JobHandlerError> {
        if !parameters.consent || parameters.disclosure_version != LIBRARY_CLEANUP_DISCLOSURE {
            return Err(JobHandlerError::new("cleanup_model_consent_required"));
        }
        let _lease = self.sources.execution_lease().await;
        let role = self
            .quality
            .prepare_quality_gated_role_execution("library_cleanup", LIBRARY_CLEANUP_QUALITY_ID)
            .await
            .map_err(|e| JobHandlerError::new(e.code()))?;
        if parameters.role_fingerprint != role.fingerprint {
            return Err(JobHandlerError::new("role_changed"));
        }
        let job = context
            .related_job(&parameters.catalog_job_id)
            .await
            .map_err(JobHandlerError::from_execution)?
            .filter(|j| {
                j.kind == super::CLEANUP_ENRICHMENT_JOB_KIND && j.status == JobStatus::Succeeded
            })
            .ok_or_else(|| JobHandlerError::new("cleanup_catalog_result_unavailable"))?;
        let plan = job
            .result
            .as_ref()
            .and_then(|r| r.get("plans"))
            .and_then(Value::as_array)
            .and_then(|plans| {
                plans
                    .iter()
                    .find(|p| p["track_id"].as_i64() == Some(parameters.track_id))
            })
            .ok_or_else(|| JobHandlerError::new("cleanup_catalog_track_unavailable"))?;
        if plan["status"].as_str() != Some("unmatched") {
            return Err(JobHandlerError::new("cleanup_identity_already_resolved"));
        }
        if !plan
            .as_object()
            .is_some_and(super::workflow::cache_is_fresh)
        {
            return Err(JobHandlerError::new("cleanup_evidence_stale"));
        }
        let id = TrackId::new(parameters.track_id)
            .map_err(|_| JobHandlerError::new("cleanup_track_invalid"))?;
        let tracks = self
            .cleanup
            .tracks(CleanupScope::All)
            .await
            .map_err(|_| JobHandlerError::new("cleanup_track_unavailable"))?;
        let track = tracks
            .iter()
            .find(|track| track.id == id)
            .ok_or_else(|| JobHandlerError::new("cleanup_track_unavailable"))?;
        let signature = cleanup_enrichment_source_signature(track).map_err(JobHandlerError::new)?;
        let folder_signature = super::discovery::indexed_folder_signature(track, tracks.iter())
            .map_err(JobHandlerError::new)?;
        let revision = self
            .cache
            .catalog_evidence_revision()
            .await
            .map_err(|_| JobHandlerError::new("cleanup_evidence_unavailable"))?;
        if plan["source_signature"].as_str() != Some(&signature)
            || plan["indexed_folder_signature"].as_str() != Some(&folder_signature)
            || plan["evidence_revision"].as_i64() != Some(revision)
        {
            return Err(JobHandlerError::new("cleanup_evidence_stale"));
        }
        let task = LibraryCleanupModelTask::new(
            track,
            serde_json::from_value(plan["candidates"].clone())
                .map_err(|_| JobHandlerError::new("cleanup_candidates_invalid"))?,
        )
        .map_err(|e| JobHandlerError::new(e.code))?;
        let request = task.request();
        let mut usage = ProviderUsageAccumulator::for_run(ModelRunManifest::new(
            context,
            &role,
            LIBRARY_CLEANUP_QUALITY_ID,
            Some(LIBRARY_CLEANUP_DISCLOSURE),
            &parameters,
            &request,
            1,
            ModelReviewDestination::LibraryCleanupReview,
        )?);
        context
            .checkpoint(usage.checkpoint())
            .await
            .map_err(JobHandlerError::from_execution)?;
        let result = execute_recorded_provider_request(
            context,
            self.transport.as_ref(),
            &role,
            &request,
            &mut usage,
        )
        .await?;
        let decision = task
            .finish(result)
            .map_err(|e| JobHandlerError::new(e.code))?;
        let current = self
            .cleanup
            .tracks(CleanupScope::All)
            .await
            .map_err(|_| JobHandlerError::new("cleanup_track_unavailable"))?;
        let current_role = self
            .quality
            .prepare_quality_gated_role_execution("library_cleanup", LIBRARY_CLEANUP_QUALITY_ID)
            .await
            .map_err(|e| JobHandlerError::new(e.code()))?;
        if current_role.fingerprint != role.fingerprint
            || self
                .cache
                .catalog_evidence_revision()
                .await
                .map_err(|_| JobHandlerError::new("cleanup_evidence_unavailable"))?
                != revision
            || current
                .iter()
                .find(|track| track.id == id)
                .and_then(|t| cleanup_enrichment_source_signature(t).ok())
                .as_deref()
                != Some(&signature)
            || super::discovery::indexed_folder_signature(track, current.iter())
                .ok()
                .as_deref()
                != Some(&folder_signature)
        {
            return Err(JobHandlerError::new("cleanup_evidence_stale"));
        }
        let mut ops = Vec::new();
        if decision.decision == CleanupModelDecision::Select
            && let Some(candidate) = decision
                .candidate_id
                .as_deref()
                .and_then(|id| task.candidate(id))
        {
            for (field, old, new) in [
                ("title", &track.metadata.title, &candidate.title),
                ("artist", &track.metadata.artist, &candidate.artist),
            ] {
                if old != new && !new.is_empty() {
                    ops.push(json!({
                    "op_id": format!("model-catalog:{}:{field}:{}", parameters.track_id, context.job_id()),
                    "track_id": parameters.track_id, "kind": "tag", "field": field, "old": old, "new": new,
                    "rules": ["model_catalog_choice"], "confidence": "low", "verified": false,
                    "evidence": {"source": "musicbrainz", "entity": "recording", "id": candidate.id, "method": "model_adjudication"},
                }));
                }
            }
        }
        Ok(
            json!({"schema_version": "assistant-library-cleanup-result/v1", "engine_id": LIBRARY_CLEANUP_ENGINE_ID,
            "track_id": parameters.track_id, "source_signature": signature, "role_fingerprint": role.fingerprint,
            "decision": decision, "ops": ops, "usage": usage.summary()}),
        )
    }
}

impl JobHandler for CleanupAiJobHandler {
    fn definition(&self) -> JobDefinition {
        JobDefinition {
            kind: CLEANUP_AI_JOB_KIND,
            schema_version: 1,
            lane: JobLane::Provider,
            restartable: false,
            checkpoint_policy: JobCheckpointPolicy::Replace,
        }
    }
    fn execute<'a>(
        &'a self,
        context: &'a JobExecutionContext,
        parameters: Map<String, Value>,
    ) -> JobHandlerFuture<'a> {
        Box::pin(async move {
            self.run(
                context,
                serde_json::from_value(Value::Object(parameters))
                    .map_err(|_| JobHandlerError::new("cleanup_model_parameters_invalid"))?,
            )
            .await
        })
    }
}
