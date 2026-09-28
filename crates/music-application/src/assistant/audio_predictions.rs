//! Bounded learned observations, kept separate from measured or catalog facts.
use super::ModelTaskError;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::BTreeSet;

pub const AUDIO_PREDICTION_CONTRACT: &str = "song-audio-candidates/v1";
pub const AUDIO_PREDICTION_MEANING: &str = "Tentative musical descriptors from audio classifiers, not verified facts or listener judgments. Labels are ranked within each head; absence from this shortlist is not negative evidence. Scores are uncalibrated sigmoid outputs, not probabilities that a tag is correct; compare ranks only within a head. top_rank_fraction is the time share in that head's top three, not time actually containing the concept. Opening and ending summarize the first and last tenth. These heads share one encoder and are not independent corroboration. Judge the descriptors' musical meaning; do not infer a setting or historical era from an instrument alone.";

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct AudioPredictionEvidence {
    pub schema_version: String,
    pub sources: Vec<AudioPredictionSource>,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct AudioPredictionSource {
    pub kind: String,
    pub model_id: String,
    pub score_kind: String,
    pub duration_seconds: f64,
    pub covered_seconds: f64,
    pub labels: Vec<RankedAudioLabel>,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct RankedAudioLabel {
    pub rank: usize,
    pub label: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub mean_score: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_score: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub top_rank_fraction: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub opening_score: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ending_score: Option<f64>,
}

impl AudioPredictionEvidence {
    pub fn parse(value: &Value) -> Result<Self, ModelTaskError> {
        let invalid = || ModelTaskError::new("model_input_invalid");
        if value.to_string().len() > 16_384 {
            return Err(invalid());
        }
        let parsed: Self = serde_json::from_value(value.clone()).map_err(|_| invalid())?;
        if parsed.schema_version != AUDIO_PREDICTION_CONTRACT
            || parsed.sources.is_empty()
            || parsed.sources.len() > 3
        {
            return Err(invalid());
        }
        let mut kinds = BTreeSet::new();
        for source in &parsed.sources {
            let expected = match source.kind.as_str() {
                "instrument" => "mtg_jamendo_instrument-discogs-effnet-1",
                "mood_theme" => "mtg_jamendo_moodtheme-discogs-effnet-1",
                "style" => "discogs-effnet-bsdynamic-1",
                _ => return Err(invalid()),
            };
            if source.model_id != expected
                || !kinds.insert(&source.kind)
                || source.score_kind != "uncalibrated_sigmoid"
                || !source.duration_seconds.is_finite()
                || !(0.0..=86_400.0).contains(&source.duration_seconds)
                || source.duration_seconds == 0.0
                || !source.covered_seconds.is_finite()
                || source.covered_seconds <= 0.0
                || source.covered_seconds > source.duration_seconds
                || source.labels.is_empty()
                || source.labels.len() > 10
            {
                return Err(invalid());
            }
            let mut labels = BTreeSet::new();
            let mut previous = f64::INFINITY;
            let layout = score_layout(&source.labels[0]);
            for (index, label) in source.labels.iter().enumerate() {
                let scores = [
                    label.mean_score,
                    label.max_score,
                    label.top_rank_fraction,
                    label.opening_score,
                    label.ending_score,
                ];
                if label.rank != index + 1
                    || label.label.trim().is_empty()
                    || label.label.len() > 128
                    || label.label.chars().any(char::is_control)
                    || !labels.insert(&label.label)
                    || scores
                        .iter()
                        .flatten()
                        .any(|v| !v.is_finite() || !(0.0..=1.0).contains(v))
                    || score_layout(label) != layout
                    || !matches!(
                        layout,
                        [false, false, false, false, false]
                            | [true, false, false, false, false]
                            | [true, true, true, true, true]
                    )
                    || label.mean_score.is_some_and(|v| v > previous)
                    || label
                        .max_score
                        .zip(label.mean_score)
                        .is_some_and(|(max, mean)| max < mean)
                    || label
                        .max_score
                        .zip(label.opening_score)
                        .is_some_and(|(max, open)| max < open)
                    || label
                        .max_score
                        .zip(label.ending_score)
                        .is_some_and(|(max, end)| max < end)
                {
                    return Err(invalid());
                }
                previous = label.mean_score.unwrap_or(previous);
            }
        }
        Ok(parsed)
    }
}

fn score_layout(label: &RankedAudioLabel) -> [bool; 5] {
    [
        label.mean_score.is_some(),
        label.max_score.is_some(),
        label.top_rank_fraction.is_some(),
        label.opening_score.is_some(),
        label.ending_score.is_some(),
    ]
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    pub(super) fn evidence() -> Value {
        json!({"schema_version":AUDIO_PREDICTION_CONTRACT,"sources":[{
            "kind":"mood_theme","model_id":"mtg_jamendo_moodtheme-discogs-effnet-1",
            "score_kind":"uncalibrated_sigmoid","duration_seconds":60.0,"covered_seconds":60.0,
            "labels":[{"rank":1,"label":"relaxing","mean_score":0.21},
                {"rank":2,"label":"meditative","mean_score":0.15}]}]})
    }

    #[test]
    fn predictions_preserve_uncalibrated_low_scores_and_do_not_invent_confidence()
    -> Result<(), Box<dyn std::error::Error>> {
        let value = evidence();
        let parsed = AudioPredictionEvidence::parse(&value)?;
        assert_eq!(serde_json::to_value(parsed)?, value);
        Ok(())
    }

    #[test]
    fn prediction_citations_are_reconstructed_and_missing_heads_cannot_be_cited()
    -> Result<(), Box<dyn std::error::Error>> {
        use crate::assistant::structured_harness::tests::model_result;
        use crate::assistant::{
            MODEL_TAGGER_OUTPUT_CONTRACT, ModelTaggerBatch, default_vocabulary_snapshot,
        };
        let input = json!({"track_id":71,"artist":"","album":"","genre":"","origin":"",
            "length_s":60.0,"audio_predictions":evidence(),
            "evidence_ids":["prediction.style","catalog.invented"]});
        let batch = ModelTaggerBatch::new(vec![input], default_vocabulary_snapshot()?)?;
        let output = json!({"schema_version":MODEL_TAGGER_OUTPUT_CONTRACT,"tracks":[{
            "track_id":1,"decisions":[{"tag_id":"mood.calm","support":"tentative",
                "evidence":["Relaxing is a tentative audio-model descriptor."],
                "evidence_ids":["prediction.mood_theme"],"contradiction_ids":[]}],"abstention_reason":null}]});
        assert!(
            batch
                .finish(model_result(output.clone()))?
                .contains_key(&71)
        );
        for id in ["prediction.style", "catalog.invented", "metadata.genre"] {
            let mut invalid = output.clone();
            invalid["tracks"][0]["decisions"][0]["evidence_ids"] = json!([id]);
            assert!(batch.finish(model_result(invalid)).is_err());
        }
        Ok(())
    }

    #[test]
    fn predictions_reject_unsupported_identity_duplicates_bad_scores_and_hidden_fields()
    -> Result<(), Box<dyn std::error::Error>> {
        for (pointer, replacement) in [
            ("/schema_version", json!("old")),
            ("/sources/0/model_id", json!("other")),
            ("/sources/0/kind", json!("verified_mood")),
            ("/sources/0/score_kind", json!("probability")),
            ("/sources/0/covered_seconds", json!(61.0)),
            ("/sources/0/labels/0/mean_score", json!(1.1)),
            ("/sources/0/labels/1/mean_score", json!(0.5)),
            ("/sources/0/labels/1/label", json!("relaxing")),
            ("/sources/0/labels/1/rank", json!(1)),
        ] {
            let mut value = evidence();
            *value.pointer_mut(pointer).ok_or("test pointer")? = replacement;
            assert!(AudioPredictionEvidence::parse(&value).is_err(), "{pointer}");
        }
        let mut value = evidence();
        value["sources"][0]["path"] = json!("private");
        assert!(AudioPredictionEvidence::parse(&value).is_err());
        let mut value = evidence();
        value["sources"][0]["labels"][0]["ending_score"] = json!(0.2);
        assert!(AudioPredictionEvidence::parse(&value).is_err());
        let source = evidence()["sources"][0].clone();
        assert!(
            AudioPredictionEvidence::parse(&json!({"schema_version":AUDIO_PREDICTION_CONTRACT,
            "sources":[source,source]}))
            .is_err()
        );
        Ok(())
    }
}
