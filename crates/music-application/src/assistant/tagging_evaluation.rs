use serde::{Deserialize, Serialize};

use super::{
    ModelTaggerBatch, ModelTaskError, StructuredModelRequest, TagQualityCase,
    TagVocabularyDocument, TagVocabularyEntry, TagVocabularyGroup, TagVocabularySnapshot,
    default_vocabulary_snapshot, plan_model_tagger_batches, vocabulary_fingerprint,
};

/// Fixed synthetic fixtures, never the operator's live vocabulary.
#[derive(Debug, Default, Clone, Copy, Eq, PartialEq, Ord, PartialOrd, Deserialize, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum TagQualityVocabulary {
    #[default]
    Default,
    Custom,
    Maximum,
}

impl TagQualityVocabulary {
    pub fn snapshot(self) -> Result<TagVocabularySnapshot, ModelTaskError> {
        if self == Self::Default {
            return default_vocabulary_snapshot();
        }
        let mut document: TagVocabularyDocument = serde_json::from_str(include_str!(
            "evaluation_suites/tagging-custom-vocabulary-v1.json"
        ))
        .map_err(|_| ModelTaskError::new("model_evaluation_vocabulary_invalid"))?;
        if self == Self::Maximum {
            // Four custom entries plus 196 explicit archival labels reach the
            // supported 200-tag limit without relying on private library data.
            for start in [1, 101] {
                document.groups.push(TagVocabularyGroup {
                    key: format!("archive_{start}"), label: format!("Archive labels {start}"),
                    description: "Fictional archive cue identifiers, requiring explicit metadata evidence.".to_owned(),
                    tags: (start..=(start + 99).min(196)).map(|index| TagVocabularyEntry {
                        id: format!("archive.cue_{index:03}"), name: format!("archive cue {index:03}"),
                        description: format!("Use only when metadata explicitly identifies archive cue {index:03}. Never infer an archive number from other evidence."),
                        aliases: Vec::new(), context_cues: Vec::new(),
                    }).collect(),
                });
            }
        }
        let document = document
            .normalized()
            .map_err(|_| ModelTaskError::new("model_evaluation_vocabulary_invalid"))?;
        let fingerprint = vocabulary_fingerprint(&document)
            .map_err(|_| ModelTaskError::new("model_evaluation_vocabulary_invalid"))?;
        Ok(TagVocabularySnapshot {
            revision: 1,
            fingerprint,
            document,
        })
    }
}

#[derive(Debug)]
pub struct PlannedTagQualityBatch {
    pub case_range: std::ops::Range<usize>,
    pub vocabulary: TagVocabularySnapshot,
    pub task: ModelTaggerBatch,
}

/// Preserve scenario order and never mix different vocabularies in a request.
pub fn plan_tag_quality_batches(
    cases: &[TagQualityCase],
    validate: impl Fn(&StructuredModelRequest) -> Result<(), ModelTaskError>,
) -> Result<Vec<PlannedTagQualityBatch>, ModelTaskError> {
    let mut result = Vec::new();
    let mut start = 0;
    while start < cases.len() {
        let fixture = cases[start].vocabulary;
        let length = cases[start..]
            .iter()
            .take_while(|case| case.vocabulary == fixture)
            .count();
        let vocabulary = fixture.snapshot()?;
        let inputs = cases[start..start + length]
            .iter()
            .map(|case| case.track.clone())
            .collect::<Vec<_>>();
        for planned in plan_model_tagger_batches(&inputs, &vocabulary, &validate)? {
            result.push(PlannedTagQualityBatch {
                case_range: start + planned.input_range.start..start + planned.input_range.end,
                vocabulary: vocabulary.clone(),
                task: planned.task,
            });
        }
        start += length;
    }
    Ok(result)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::assistant::{
        MODEL_TAGGER_OUTPUT_CONTRACT, ProviderAttemptOutcome, StructuredModelResult,
        TagQualityEvaluationResult, tag_quality_suite,
    };
    use serde_json::{Value, json};

    #[test]
    fn batches_isolate_vocabulary_and_preserve_retest_order()
    -> Result<(), Box<dyn std::error::Error>> {
        let suite = tag_quality_suite()?;
        let selected = [50, 0, 55, 52]
            .into_iter()
            .map(|index| suite.cases[index].clone())
            .collect::<Vec<_>>();
        let planned = plan_tag_quality_batches(&selected, |_| Ok(()))?;
        assert_eq!(planned.len(), 4);
        for (index, planned) in planned.iter().enumerate() {
            assert_eq!(planned.case_range, index..index + 1);
            assert_eq!(
                planned.vocabulary.fingerprint,
                selected[index].vocabulary.snapshot()?.fingerprint
            );
            let input: Value = serde_json::from_str(&planned.task.request(false).user_prompt)?;
            assert_eq!(
                input["tracks"][0]["track_id"],
                selected[index].track["track_id"]
            );
            let supplied = input["vocabulary_groups"]
                .as_array()
                .ok_or("groups missing")?
                .iter()
                .flat_map(|group| group["tags"].as_array().into_iter().flatten())
                .count();
            assert_eq!(supplied, planned.vocabulary.entries().count());
        }
        assert_eq!(
            TagQualityVocabulary::Custom.snapshot()?.entries().count(),
            4
        );
        assert_eq!(
            TagQualityVocabulary::Maximum.snapshot()?.entries().count(),
            200
        );
        Ok(())
    }

    #[test]
    fn expected_labels_round_trip_and_each_vocabulary_keeps_its_quality_gate()
    -> Result<(), Box<dyn std::error::Error>> {
        let suite = tag_quality_suite()?;
        let mut results = Vec::new();
        for case in &suite.cases {
            let vocabulary = case.vocabulary.snapshot()?;
            let ids = case
                .required_tags
                .iter()
                .map(|name| {
                    vocabulary
                        .entries()
                        .find(|entry| &entry.name == name)
                        .map(|entry| entry.id.clone())
                        .ok_or("expected tag missing")
                })
                .collect::<Result<Vec<_>, _>>()?;
            let id = case.track["track_id"].as_i64().ok_or("track ID missing")?;
            let batch = ModelTaggerBatch::new(vec![case.track.clone()], vocabulary.clone())?;
            let profiles = batch.finish(StructuredModelResult {
                outcome: ProviderAttemptOutcome::ResponseReceived,
                succeeded: true,
                error_code: None,
                payload: Some(
                    json!({"schema_version": MODEL_TAGGER_OUTPUT_CONTRACT, "tracks": [{
                        "track_id": id, "tag_ids": ids, "confidence": case.allowed_confidences[0],
                        "evidence": ["Fixed synthetic expected-label fixture."]
                    }]}),
                ),
                provider_model_id: None,
                finish_reason: None,
                input_tokens: None,
                output_tokens: None,
            })?;
            let result = case.assess(Ok(profiles.get(&id).ok_or("profile missing")?), &vocabulary);
            assert!(result.passed, "{}: {:?}", case.id, result.failures);
            results.push(result);
        }
        assert!(TagQualityEvaluationResult::summarize(&suite, results.clone())?.passed);
        // 55/56 is above the aggregate threshold, but 4/5 custom cases is not.
        results[50].passed = false;
        results[50]
            .failures
            .push("Missing required tags: quiet focus".to_owned());
        let summary = TagQualityEvaluationResult::summarize(&suite, results)?;
        assert_eq!(summary.passed_cases, 55);
        assert!(!summary.passed);
        assert!(
            summary
                .vocabulary_results
                .iter()
                .any(|group| group.vocabulary == TagQualityVocabulary::Custom && !group.passed)
        );
        assert!(
            summary
                .vocabulary_results
                .iter()
                .any(|group| group.vocabulary == TagQualityVocabulary::Default && group.passed)
        );
        Ok(())
    }
}
