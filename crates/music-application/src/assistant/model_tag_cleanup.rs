use std::collections::{BTreeMap, BTreeSet};

use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};

use super::structured_harness::{
    ModelTaskError, StructuredTaskDefinition, build_structured_request, safe_execution_error,
    truncate_chars,
};
use super::{
    StructuredModelRequest, StructuredModelResult, TagUsage, TagVocabularySnapshot,
    build_cleanup_preview, default_vocabulary, vocabulary_fingerprint,
};

pub const MODEL_TAG_CLEANUP_INPUT_CONTRACT: &str = "assistant-model-tag-cleanup-input/v3";
pub const MODEL_TAG_CLEANUP_OUTPUT_CONTRACT: &str = "assistant-model-tag-cleanup-output/v2";
pub const MODEL_TAG_CLEANUP_EVALUATION_CONTRACT: &str = "assistant-model-tag-cleanup-evaluation/v2";
pub const MODEL_TAG_CLEANUP_ENGINE_ID: &str = "model-tag-cleanup/v3";
pub const TAG_CLEANUP_QUALITY_SUITE_ID: &str = "controlled-vocabulary-cleanup-baseline-v6";
pub const MAX_MODEL_CLEANUP_TAGS: usize = 500;
pub const MAX_MODEL_CLEANUP_SUGGESTIONS: usize = 100;
pub const MODEL_TAG_CLEANUP_BATCH_SIZE: usize = 20;

const TAG_CLEANUP_TASK: StructuredTaskDefinition = StructuredTaskDefinition {
    task_id: "assistant-tag-cleanup",
    role: "A conservative catalog normalizer for operator-owned music tags.",
    objective: "Classify every unresolved source as exactly one canonical tag ID or no safe match after deterministic aliases, spelling, and plurals are removed.",
    untrusted_data: &[
        "candidate source tags",
        "canonical tag names and descriptions",
    ],
    rules: &[
        "Return every candidate source_id exactly once and no other source IDs. Preserve the input order.",
        "target_tag_id must be an exact ID from canonical_tags or null. Never return a tag name, source text, or invented ID as the target.",
        "The server already handled declared aliases and unambiguous spelling and plurals. Choose a target only for a clear semantic synonym that preserves useful distinctions.",
        "The target definition must cover the complete source meaning, not merely one word. A multiword source may map when one definition covers all meaningful parts; otherwise use null instead of discarding a mood, period, setting, scene, or other modifier.",
        "Do not merge related but meaningfully distinct settings, scenes, or moods. Use null whenever track context would be needed to decide safely.",
        "Track counts indicate adoption only; popularity does not make two meanings equivalent. Definitions are authoritative when labels could overlap.",
        "Do not stop after the first match. Evaluate each source independently even when earlier sources map to the same target.",
        "At most remaining_suggestion_slots decisions may use a non-null target_tag_id. All other sources must still receive an explicit null decision.",
        "reason must be a short catalog-level explanation and confidence must reflect how unambiguous the decision is.",
    ],
};

#[derive(Debug, Clone, Eq, PartialEq, Deserialize, Serialize, schemars::JsonSchema)]
#[serde(rename_all = "lowercase")]
pub enum CleanupConfidence {
    High,
    Medium,
    Low,
}

impl CleanupConfidence {
    #[must_use]
    pub const fn as_str(&self) -> &'static str {
        match self {
            Self::High => "high",
            Self::Medium => "medium",
            Self::Low => "low",
        }
    }
}

#[derive(Debug, Clone, Eq, PartialEq, Serialize)]
pub struct ModelTagCleanupSuggestion {
    pub source: String,
    pub target: String,
    pub confidence: CleanupConfidence,
    pub reason: String,
}

#[must_use]
pub fn model_tag_cleanup_suggestion_id(
    role_fingerprint: &str,
    catalog_signature: &str,
    vocabulary_fingerprint: &str,
    suggestion: &ModelTagCleanupSuggestion,
) -> String {
    let payload = format!(
        "{MODEL_TAG_CLEANUP_ENGINE_ID}\0{role_fingerprint}\0{catalog_signature}\0{vocabulary_fingerprint}\0{}\0{}",
        suggestion.source, suggestion.target
    );
    format!("{:x}", Sha256::digest(payload.as_bytes()))
}

#[derive(Debug)]
pub struct ModelTagCleanupTask {
    vocabulary: TagVocabularySnapshot,
    indexed_sources: Vec<(String, TagUsage)>,
    local_suggestions: Vec<ModelTagCleanupSuggestion>,
    model_suggestions: Vec<ModelTagCleanupSuggestion>,
    offset: usize,
}

impl ModelTagCleanupTask {
    pub fn new(
        usage: &[TagUsage],
        vocabulary: TagVocabularySnapshot,
    ) -> Result<Self, ModelTaskError> {
        if usage.len() > MAX_MODEL_CLEANUP_TAGS {
            return Err(ModelTaskError::new("catalog_too_large"));
        }
        let preview = build_cleanup_preview(usage, &vocabulary)
            .map_err(|_| ModelTaskError::new("tag_cleanup_preview_failed"))?;
        let all_local_sources = preview
            .suggestions
            .iter()
            .map(|suggestion| suggestion.source.clone())
            .collect::<BTreeSet<_>>();
        let local_suggestions = preview
            .suggestions
            .into_iter()
            .take(MAX_MODEL_CLEANUP_SUGGESTIONS)
            .map(|suggestion| ModelTagCleanupSuggestion {
                source: suggestion.source,
                target: suggestion.target,
                confidence: CleanupConfidence::High,
                reason: suggestion.reason,
            })
            .collect::<Vec<_>>();
        let canonical_names = vocabulary
            .entries()
            .map(|entry| entry.name.as_str())
            .collect::<BTreeSet<_>>();
        let indexed_sources = usage
            .iter()
            .filter(|item| {
                !canonical_names.contains(item.tag.as_str())
                    && !all_local_sources.contains(&item.tag)
            })
            .enumerate()
            .map(|(index, item)| (format!("source-{:03}", index + 1), item.clone()))
            .collect();
        Ok(Self {
            vocabulary,
            indexed_sources,
            local_suggestions,
            model_suggestions: Vec::new(),
            offset: 0,
        })
    }

    #[must_use]
    pub fn total_model_batches(&self) -> usize {
        self.indexed_sources
            .len()
            .div_ceil(MODEL_TAG_CLEANUP_BATCH_SIZE)
    }

    #[must_use]
    pub fn completed_model_batches(&self) -> usize {
        self.offset.div_ceil(MODEL_TAG_CLEANUP_BATCH_SIZE)
    }

    #[must_use]
    pub fn next_request(&self) -> Option<StructuredModelRequest> {
        if self.offset >= self.indexed_sources.len()
            || self.local_suggestions.len() + self.model_suggestions.len()
                >= MAX_MODEL_CLEANUP_SUGGESTIONS
        {
            return None;
        }
        let batch = self.current_batch().to_vec();
        let source_ids = batch
            .iter()
            .map(|(source_id, _)| source_id.clone())
            .collect::<Vec<_>>();
        let tag_ids = self
            .vocabulary
            .entries()
            .map(|entry| entry.id.clone())
            .collect::<Vec<_>>();
        let canonical_tags = self
            .vocabulary
            .document
            .groups
            .iter()
            .flat_map(|group| {
                group.tags.iter().map(|tag| {
                    json!({
                        "tag_id": tag.id,
                        "name": tag.name,
                        "group": group.label,
                        "description": tag.description,
                    })
                })
            })
            .collect::<Vec<_>>();
        Some(build_structured_request(
            &TAG_CLEANUP_TASK,
            json!({
                "schema_version": MODEL_TAG_CLEANUP_INPUT_CONTRACT,
                "canonical_tags": canonical_tags,
                "candidate_sources": batch.iter().map(|(source_id, item)| json!({
                    "source_id": source_id,
                    "tag": item.tag,
                    "track_count": item.track_count,
                })).collect::<Vec<_>>(),
                "remaining_suggestion_slots": MAX_MODEL_CLEANUP_SUGGESTIONS
                    .saturating_sub(self.local_suggestions.len() + self.model_suggestions.len()),
            }),
            cleanup_output_schema(&source_ids, &tag_ids),
            json!({
                "schema_version": MODEL_TAG_CLEANUP_OUTPUT_CONTRACT,
                "decisions": source_ids.iter().map(|source_id| json!({
                    "source_id": source_id,
                    "target_tag_id": null,
                    "confidence": "low",
                    "reason": "No safe canonical match is established.",
                })).collect::<Vec<_>>(),
            }),
            8_000,
        ))
    }

    pub fn accept(&mut self, result: StructuredModelResult) -> Result<(), ModelTaskError> {
        if self.next_request().is_none() {
            return Err(ModelTaskError::new("model_cleanup_already_complete"));
        }
        if !result.succeeded {
            return Err(ModelTaskError::new(safe_execution_error(
                result.error_code.as_deref(),
            )));
        }
        if matches!(
            result.finish_reason.as_deref(),
            Some("length" | "max_tokens")
        ) {
            return Err(ModelTaskError::new("model_output_incomplete"));
        }
        let payload = bound_cleanup_reasons(
            result
                .payload
                .ok_or_else(|| ModelTaskError::new("model_execution_failed"))?,
        );
        let output: ModelTagCleanupOutput = serde_json::from_value(payload)
            .map_err(|error| ModelTaskError::invalid_output(error.to_string()))?;
        if output.schema_version != MODEL_TAG_CLEANUP_OUTPUT_CONTRACT
            || output.decisions.is_empty()
            || output.decisions.len() > MAX_MODEL_CLEANUP_TAGS
        {
            return Err(ModelTaskError::invalid_output(
                "invalid cleanup output fields",
            ));
        }
        let batch = self.current_batch().to_vec();
        let expected_ids = batch
            .iter()
            .map(|(source_id, _)| source_id.as_str())
            .collect::<Vec<_>>();
        let returned_ids = output
            .decisions
            .iter()
            .map(|decision| decision.source_id.as_str())
            .collect::<Vec<_>>();
        if returned_ids != expected_ids {
            return Err(ModelTaskError::new("model_output_source_set_mismatch"));
        }
        let tags_by_id = self
            .vocabulary
            .entries()
            .map(|entry| (entry.id.as_str(), entry.name.as_str()))
            .collect::<BTreeMap<_, _>>();
        for (decision, (_, source)) in output.decisions.into_iter().zip(&batch) {
            validate_cleanup_decision(&decision)?;
            let ModelTagCleanupTarget::Tag(target_id) = decision.target_tag_id else {
                continue;
            };
            let target = tags_by_id
                .get(target_id.as_str())
                .ok_or_else(|| ModelTaskError::new("model_output_unknown_target"))?;
            self.model_suggestions.push(ModelTagCleanupSuggestion {
                source: source.tag.clone(),
                target: (*target).to_owned(),
                confidence: decision.confidence,
                reason: decision.reason,
            });
            if self.local_suggestions.len() + self.model_suggestions.len()
                > MAX_MODEL_CLEANUP_SUGGESTIONS
            {
                return Err(ModelTaskError::new("model_output_too_many_suggestions"));
            }
        }
        self.offset += batch.len();
        Ok(())
    }

    #[must_use]
    pub fn finish(self) -> Option<Vec<ModelTagCleanupSuggestion>> {
        (self.next_request().is_none()).then(|| {
            self.local_suggestions
                .into_iter()
                .chain(self.model_suggestions)
                .collect()
        })
    }

    fn current_batch(&self) -> &[(String, TagUsage)] {
        let end = (self.offset + MODEL_TAG_CLEANUP_BATCH_SIZE).min(self.indexed_sources.len());
        self.indexed_sources
            .get(self.offset..end)
            .unwrap_or_default()
    }
}

#[derive(Debug, Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
struct ModelTagCleanupOutput {
    schema_version: String,
    decisions: Vec<ModelTagCleanupDecision>,
}

#[derive(Debug, Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
struct ModelTagCleanupDecision {
    source_id: String,
    target_tag_id: ModelTagCleanupTarget,
    confidence: CleanupConfidence,
    reason: String,
}

// An explicit null means abstention. Option would also accept an omitted field.
#[derive(Debug, Deserialize, schemars::JsonSchema)]
#[serde(untagged)]
enum ModelTagCleanupTarget {
    Tag(String),
    Abstain,
}

#[derive(Debug, Clone, Eq, PartialEq, Ord, PartialOrd, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct TagCleanupPair {
    pub source: String,
    pub target: String,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
struct GeneratedCleanupTags {
    count: usize,
    prefix: String,
    #[serde(default = "default_track_count")]
    track_count: u64,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct TagCleanupQualityCase {
    pub id: String,
    pub description: String,
    #[serde(default)]
    pub used_tags: Vec<TagUsageWire>,
    #[serde(default)]
    generated_used_tags: Option<GeneratedCleanupTags>,
    #[serde(default)]
    pub required_pairs: Vec<TagCleanupPair>,
    #[serde(default)]
    pub forbidden_pairs: Vec<TagCleanupPair>,
    #[serde(default = "maximum_cleanup_suggestions")]
    pub maximum_suggestions: usize,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct TagUsageWire {
    pub tag: String,
    pub track_count: u64,
}

impl TagCleanupQualityCase {
    #[must_use]
    pub fn usage(&self) -> Vec<TagUsage> {
        let mut usage = self
            .used_tags
            .iter()
            .map(|item| TagUsage {
                tag: item.tag.clone(),
                track_count: item.track_count,
            })
            .collect::<Vec<_>>();
        if let Some(generated) = &self.generated_used_tags {
            usage.extend((1..=generated.count).map(|index| TagUsage {
                tag: format!("{} {index:02}", generated.prefix),
                track_count: generated.track_count,
            }));
        }
        usage
    }

    #[must_use]
    pub fn assess(
        &self,
        result: Result<Vec<ModelTagCleanupSuggestion>, ModelTaskError>,
    ) -> TagCleanupQualityCaseResult {
        let mut failures = Vec::new();
        let actual = match result {
            Ok(suggestions) => suggestions
                .into_iter()
                .map(|suggestion| TagCleanupPair {
                    source: suggestion.source,
                    target: suggestion.target,
                })
                .collect::<BTreeSet<_>>(),
            Err(error) => {
                let mut failure = format!("Cleanup model error: {}", error.code);
                if let Some(diagnostic) = error.diagnostic {
                    failure.push_str(&format!(" ({diagnostic})"));
                }
                failures.push(failure);
                BTreeSet::new()
            }
        };
        let required = self.required_pairs.iter().cloned().collect::<BTreeSet<_>>();
        let forbidden = self
            .forbidden_pairs
            .iter()
            .cloned()
            .collect::<BTreeSet<_>>();
        let missing = required.difference(&actual).cloned().collect::<Vec<_>>();
        if !missing.is_empty() {
            failures.push(format!(
                "Missing required cleanup pairs: {}",
                format_pairs(&missing)
            ));
        }
        let returned_forbidden = forbidden.intersection(&actual).cloned().collect::<Vec<_>>();
        if !returned_forbidden.is_empty() {
            failures.push(format!(
                "Returned forbidden cleanup pairs: {}",
                format_pairs(&returned_forbidden)
            ));
        }
        if actual.len() > self.maximum_suggestions {
            failures.push(format!(
                "Returned too many cleanup suggestions: expected at most {}",
                self.maximum_suggestions
            ));
        }
        TagCleanupQualityCaseResult {
            id: self.id.clone(),
            description: self.description.clone(),
            passed: failures.is_empty(),
            suggestions: actual.into_iter().collect(),
            failures,
        }
    }
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct TagCleanupQualitySuite {
    pub schema_version: String,
    pub id: String,
    pub cases: Vec<TagCleanupQualityCase>,
}

#[derive(Debug, Clone, Serialize)]
pub struct TagCleanupQualityCaseResult {
    pub id: String,
    pub description: String,
    pub passed: bool,
    pub suggestions: Vec<TagCleanupPair>,
    pub failures: Vec<String>,
}

#[derive(Debug, Serialize)]
pub struct TagCleanupQualityEvaluationResult {
    pub schema_version: &'static str,
    pub suite_id: String,
    pub engine_id: &'static str,
    pub passed: bool,
    pub passed_cases: u32,
    pub total_cases: u32,
    pub cases: Vec<TagCleanupQualityCaseResult>,
}

impl TagCleanupQualityEvaluationResult {
    #[must_use]
    pub fn from_cases(
        suite: &TagCleanupQualitySuite,
        cases: Vec<TagCleanupQualityCaseResult>,
    ) -> Self {
        let total_cases = u32::try_from(cases.len()).unwrap_or(u32::MAX);
        let passed_cases =
            u32::try_from(cases.iter().filter(|case| case.passed).count()).unwrap_or(u32::MAX);
        Self {
            schema_version: "assistant-model-tag-cleanup-quality-result/v1",
            suite_id: suite.id.clone(),
            engine_id: MODEL_TAG_CLEANUP_ENGINE_ID,
            passed: passed_cases == total_cases,
            passed_cases,
            total_cases,
            cases,
        }
    }
}

pub fn tag_cleanup_quality_suite() -> Result<TagCleanupQualitySuite, ModelTaskError> {
    let suite: TagCleanupQualitySuite =
        serde_json::from_str(include_str!("evaluation_suites/tag-cleanup-v1.json"))
            .map_err(|error| ModelTaskError::invalid_output(error.to_string()))?;
    if suite.schema_version != MODEL_TAG_CLEANUP_EVALUATION_CONTRACT
        || suite.id != TAG_CLEANUP_QUALITY_SUITE_ID
        || suite.cases.is_empty()
        || suite.cases.len() > 100
        || suite
            .cases
            .iter()
            .map(|case| case.id.as_str())
            .collect::<BTreeSet<_>>()
            .len()
            != suite.cases.len()
    {
        return Err(ModelTaskError::new("model_evaluation_suite_invalid"));
    }
    Ok(suite)
}

pub fn default_vocabulary_snapshot() -> Result<TagVocabularySnapshot, ModelTaskError> {
    let document =
        default_vocabulary().map_err(|_| ModelTaskError::new("tag_vocabulary_unavailable"))?;
    let fingerprint = vocabulary_fingerprint(&document)
        .map_err(|_| ModelTaskError::new("tag_vocabulary_unavailable"))?;
    Ok(TagVocabularySnapshot {
        revision: 0,
        fingerprint,
        document,
    })
}

fn validate_cleanup_decision(decision: &ModelTagCleanupDecision) -> Result<(), ModelTaskError> {
    let valid_source_id = decision.source_id.len() == 10
        && decision.source_id.starts_with("source-")
        && decision.source_id[7..]
            .bytes()
            .all(|byte| byte.is_ascii_digit());
    if !valid_source_id
        || matches!(&decision.target_tag_id, ModelTagCleanupTarget::Tag(value)
            if !(2..=64).contains(&value.chars().count()))
        || decision.reason.is_empty()
        || decision.reason.chars().count() > 512
    {
        return Err(ModelTaskError::invalid_output(
            "invalid cleanup decision fields",
        ));
    }
    Ok(())
}

fn bound_cleanup_reasons(mut payload: Value) -> Value {
    let Some(decisions) = payload
        .as_object_mut()
        .and_then(|object| object.get_mut("decisions"))
        .and_then(Value::as_array_mut)
    else {
        return payload;
    };
    for decision in decisions {
        if let Some(Value::String(reason)) = decision
            .as_object_mut()
            .and_then(|object| object.get_mut("reason"))
        {
            *reason = truncate_chars(reason, 512);
        }
    }
    payload
}

fn cleanup_output_schema(source_ids: &[String], tag_ids: &[String]) -> Value {
    let mut schema = super::structured_harness::output_schema::<ModelTagCleanupOutput>();
    schema["properties"]["schema_version"]["const"] = json!(MODEL_TAG_CLEANUP_OUTPUT_CONTRACT);
    let decisions = &mut schema["properties"]["decisions"];
    decisions["minItems"] = json!(source_ids.len());
    decisions["maxItems"] = json!(source_ids.len());
    let properties = &mut decisions["items"]["properties"];
    properties["source_id"]["enum"] = json!(source_ids);
    let target = &mut properties["target_tag_id"]["anyOf"][0];
    target["minLength"] = json!(2);
    target["maxLength"] = json!(64);
    target["enum"] = json!(tag_ids);
    properties["reason"]["minLength"] = json!(1);
    properties["reason"]["maxLength"] = json!(512);
    schema
}

fn format_pairs(pairs: &[TagCleanupPair]) -> String {
    pairs
        .iter()
        .map(|pair| format!("{} -> {}", pair.source, pair.target))
        .collect::<Vec<_>>()
        .join(", ")
}

const fn default_track_count() -> u64 {
    1
}

const fn maximum_cleanup_suggestions() -> usize {
    MAX_MODEL_CLEANUP_SUGGESTIONS
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::{ModelTagCleanupTask, default_vocabulary_snapshot, tag_cleanup_quality_suite};
    use crate::assistant::{StructuredModelResult, TagUsage};

    #[test]
    fn derived_schema_requires_explicit_cleanup_decisions() -> Result<(), Box<dyn std::error::Error>>
    {
        use crate::assistant::structured_harness::tests::{assert_output_contract, model_result};
        let make_task = || {
            ModelTagCleanupTask::new(
                &[TagUsage {
                    tag: "clue hunting".to_owned(),
                    track_count: 2,
                }],
                default_vocabulary_snapshot()?,
            )
        };
        let schema = make_task()?
            .next_request()
            .and_then(|request| request.output_schema)
            .ok_or("missing schema")?;
        for target in [json!(null), json!("scene.investigation")] {
            let valid = json!({"schema_version": super::MODEL_TAG_CLEANUP_OUTPUT_CONTRACT,
                "decisions": [{"source_id": "source-001", "target_tag_id": target,
                    "confidence": "high", "reason": "Catalog-level decision."}]});
            assert_output_contract(&schema, &valid, |value| {
                make_task().is_ok_and(|mut task| task.accept(model_result(value)).is_ok())
            })?;
            for (field, value) in [
                ("source_id", json!("source-999")),
                ("target_tag_id", json!("invented")),
                ("confidence", json!("certain")),
            ] {
                let mut invalid = valid.clone();
                invalid["decisions"][0][field] = value;
                assert!(!jsonschema::is_valid(&schema, &invalid));
                assert!(make_task()?.accept(model_result(invalid)).is_err());
            }
        }
        Ok(())
    }

    #[test]
    fn cleanup_requires_one_ordered_decision_per_source() -> Result<(), Box<dyn std::error::Error>>
    {
        let mut task = ModelTagCleanupTask::new(
            &[TagUsage {
                tag: "clue hunting".to_owned(),
                track_count: 2,
            }],
            default_vocabulary_snapshot()?,
        )?;
        assert!(task.next_request().is_some());
        task.accept(StructuredModelResult {
            outcome: crate::assistant::ProviderAttemptOutcome::ResponseReceived,
            succeeded: true,
            error_code: None,
            payload: Some(json!({
                "schema_version": "assistant-model-tag-cleanup-output/v2",
                "decisions": [{
                    "source_id": "source-001",
                    "target_tag_id": "scene.investigation",
                    "confidence": "high",
                    "reason": "A direct semantic synonym."
                }]
            })),
            provider_model_id: None,
            finish_reason: Some("stop".to_owned()),
            input_tokens: None,
            output_tokens: None,
        })?;
        let suggestions = task.finish().ok_or("cleanup did not finish")?;
        assert_eq!(suggestions.len(), 1);
        assert_eq!(suggestions[0].target, "investigation");
        Ok(())
    }

    #[test]
    fn bundled_cleanup_suite_materializes_boundary_cases() -> Result<(), Box<dyn std::error::Error>>
    {
        let suite = tag_cleanup_quality_suite()?;
        assert_eq!(suite.cases.len(), 15);
        let boundary = suite
            .cases
            .iter()
            .find(|case| case.id == "production-batch-boundary")
            .ok_or("missing boundary case")?;
        assert_eq!(boundary.usage().len(), 20);
        Ok(())
    }
}
