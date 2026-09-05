use std::collections::BTreeSet;

use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

use super::structured_harness::{
    ModelTaskError, StructuredTaskDefinition, build_structured_request, safe_execution_error,
    truncate_chars,
};
use super::{StructuredModelRequest, StructuredModelResult};

pub const EQ_DRAFT_INPUT_CONTRACT: &str = "assistant-eq-draft-input/v2";
pub const EQ_DRAFT_OUTPUT_CONTRACT: &str = "assistant-eq-draft-output/v1";
pub const EQ_DRAFT_ENGINE_ID: &str = "model-graphic-eq/v2";
pub const EQ_QUALITY_SUITE_ID: &str = "graphic-eq-safety-baseline-v4";
pub const EQ_FREQUENCIES: [u32; 10] = [32, 64, 125, 250, 500, 1_000, 2_000, 4_000, 8_000, 16_000];

const EQ_TASK: StructuredTaskDefinition = StructuredTaskDefinition {
    task_id: "assistant-eq-draft",
    role: "A conservative graphic-EQ refinement engine.",
    objective: "Refine a deterministic ten-band baseline within server-owned safety envelopes for later human listening review.",
    untrusted_data: &["goal"],
    rules: &[
        "Use the supplied bands in their exact order. Start from local_guidance.baseline_gain_db and change a band only when the sound goal supports the change.",
        "Every gain must stay within that band's minimum_gain_db and maximum_gain_db and use 0.5 dB steps. Prefer the smallest effective change.",
        "Avoid broad boosts, extreme bass, excessive presence, or curves likely to reduce headroom. This is a review draft, not a promise about a recording or playback system.",
        "If no local intent rule matched, remain close to neutral and use cautions to explain genuine ambiguity.",
        "rationale briefly relates the curve to the stated sound goal. cautions contains only practical listening or headroom checks, not hidden reasoning.",
    ],
};

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct EqBandGuidance {
    pub frequency_hz: u32,
    pub baseline_gain_db: f64,
    pub minimum_gain_db: f64,
    pub maximum_gain_db: f64,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct EqLocalGuidance {
    pub method: &'static str,
    pub matched_rules: Vec<String>,
    pub bands: Vec<EqBandGuidance>,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct EqPresetBand {
    pub frequency: u32,
    pub gain: f64,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct EqPresetDraft {
    pub name: String,
    pub goal: String,
    pub bands: Vec<EqPresetBand>,
    pub rationale: String,
    pub cautions: Vec<String>,
}

#[derive(Debug)]
pub struct EqDraftTask {
    name: String,
    goal: String,
    guidance: EqLocalGuidance,
}

impl EqDraftTask {
    pub fn new(name: &str, goal: &str) -> Result<Self, ModelTaskError> {
        if name.is_empty()
            || name.chars().count() > 128
            || !(2..=1_000).contains(&goal.chars().count())
        {
            return Err(ModelTaskError::new("model_input_invalid"));
        }
        Ok(Self {
            name: name.to_owned(),
            goal: goal.to_owned(),
            guidance: build_local_eq_guidance(goal),
        })
    }

    #[must_use]
    pub fn request(&self) -> StructuredModelRequest {
        let gains = self
            .guidance
            .bands
            .iter()
            .map(|band| band.baseline_gain_db)
            .collect::<Vec<_>>();
        build_structured_request(
            &EQ_TASK,
            json!({
                "schema_version": EQ_DRAFT_INPUT_CONTRACT,
                "goal": self.goal,
                "band_frequencies_hz": EQ_FREQUENCIES,
                "gain_min_db": -12,
                "gain_max_db": 12,
                "gain_step_db": 0.5,
                "local_guidance": self.guidance,
            }),
            eq_output_schema(),
            json!({
                "schema_version": EQ_DRAFT_OUTPUT_CONTRACT,
                "gains_db": gains,
                "rationale": "Conservative refinement of the local baseline.",
                "cautions": ["Review on the intended speakers at matched volume."],
            }),
            2_000,
        )
    }

    pub fn finish(&self, result: StructuredModelResult) -> Result<EqPresetDraft, ModelTaskError> {
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
        let payload = bound_incidental_text(
            result
                .payload
                .ok_or_else(|| ModelTaskError::new("model_execution_failed"))?,
        );
        let output: EqDraftOutput = serde_json::from_value(payload)
            .map_err(|error| ModelTaskError::invalid_output(error.to_string()))?;
        validate_output(&output)?;
        if output
            .gains_db
            .iter()
            .zip(&self.guidance.bands)
            .any(|(gain, band)| *gain < band.minimum_gain_db || *gain > band.maximum_gain_db)
        {
            return Err(ModelTaskError::new("model_output_outside_local_envelope"));
        }
        Ok(EqPresetDraft {
            name: self.name.clone(),
            goal: self.goal.clone(),
            bands: EQ_FREQUENCIES
                .iter()
                .zip(output.gains_db)
                .map(|(frequency, gain)| EqPresetBand {
                    frequency: *frequency,
                    gain,
                })
                .collect(),
            rationale: output.rationale,
            cautions: output.cautions,
        })
    }
}

#[derive(Debug, Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
struct EqDraftOutput {
    schema_version: String,
    gains_db: Vec<f64>,
    rationale: String,
    cautions: Vec<String>,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct EqBandExpectation {
    pub frequency_hz: u32,
    pub minimum_gain_db: f64,
    pub maximum_gain_db: f64,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct EqQualityCase {
    pub id: String,
    pub description: String,
    pub goal: String,
    pub expectations: Vec<EqBandExpectation>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct EqQualitySuite {
    pub schema_version: String,
    pub id: String,
    pub cases: Vec<EqQualityCase>,
}

#[derive(Debug, Clone, Serialize)]
pub struct EqQualityCaseResult {
    pub id: String,
    pub description: String,
    pub passed: bool,
    pub failures: Vec<String>,
}

#[derive(Debug, Serialize)]
pub struct EqQualityEvaluationResult {
    pub schema_version: &'static str,
    pub suite_id: String,
    pub engine_id: &'static str,
    pub passed: bool,
    pub passed_cases: u32,
    pub total_cases: u32,
    pub cases: Vec<EqQualityCaseResult>,
}

impl EqQualityCase {
    pub fn assess(&self, result: Result<EqPresetDraft, ModelTaskError>) -> EqQualityCaseResult {
        let mut failures = Vec::new();
        match result {
            Ok(draft) => {
                for expected in &self.expectations {
                    let actual = draft
                        .bands
                        .iter()
                        .find(|band| band.frequency == expected.frequency_hz)
                        .map(|band| band.gain);
                    if actual.is_none_or(|actual| {
                        actual < expected.minimum_gain_db || actual > expected.maximum_gain_db
                    }) {
                        failures.push(format!(
                            "{} Hz must be between {} and {} dB.",
                            expected.frequency_hz,
                            compact_number(expected.minimum_gain_db),
                            compact_number(expected.maximum_gain_db),
                        ));
                    }
                }
            }
            Err(error) => {
                let mut failure = format!("EQ model error: {}", error.code);
                if let Some(diagnostic) = error.diagnostic {
                    failure.push_str(&format!(" ({diagnostic})"));
                }
                failures.push(failure);
            }
        }
        EqQualityCaseResult {
            id: self.id.clone(),
            description: self.description.clone(),
            passed: failures.is_empty(),
            failures,
        }
    }
}

impl EqQualityEvaluationResult {
    #[must_use]
    pub fn from_cases(suite: &EqQualitySuite, cases: Vec<EqQualityCaseResult>) -> Self {
        let total_cases = u32::try_from(cases.len()).unwrap_or(u32::MAX);
        let passed_cases =
            u32::try_from(cases.iter().filter(|case| case.passed).count()).unwrap_or(u32::MAX);
        Self {
            schema_version: "assistant-eq-quality-result/v1",
            suite_id: suite.id.clone(),
            engine_id: EQ_DRAFT_ENGINE_ID,
            passed: passed_cases == total_cases,
            passed_cases,
            total_cases,
            cases,
        }
    }
}

pub fn eq_quality_suite() -> Result<EqQualitySuite, ModelTaskError> {
    let suite: EqQualitySuite =
        serde_json::from_str(include_str!("evaluation_suites/eq-assistant-v1.json"))
            .map_err(|error| ModelTaskError::invalid_output(error.to_string()))?;
    if suite.schema_version != "assistant-eq-quality/v1"
        || suite.id != EQ_QUALITY_SUITE_ID
        || suite.cases.is_empty()
        || suite.cases.len() > 20
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

#[must_use]
pub fn build_local_eq_guidance(goal: &str) -> EqLocalGuidance {
    const RULES: [(&str, &[&str], [f64; 10]); 5] = [
        (
            "warmth",
            &["warm", "warmer", "wooden", "body", "intimate", "tavern"],
            [0.0, 0.5, 1.0, 1.5, 0.5, 0.0, -0.5, -0.5, -1.0, -0.5],
        ),
        (
            "reduce-harshness",
            &[
                "harsh", "piercing", "brittle", "fatigue", "shrill", "sibilant",
            ],
            [0.0, 0.0, 0.0, 0.0, 0.0, 0.0, -0.5, -2.0, -1.0, -0.5],
        ),
        (
            "clarity",
            &[
                "clarity",
                "clear",
                "dialogue",
                "understandable",
                "definition",
            ],
            [-0.5, -0.5, 0.0, -0.5, -0.5, 0.0, 1.0, 0.5, 0.0, 0.0],
        ),
        (
            "bass-weight",
            &["bass", "low", "low-end", "weight", "thump"],
            [1.0, 1.5, 1.0, 0.5, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0],
        ),
        (
            "reduce-mud",
            &["muddy", "mud", "boxy", "boomy", "boom"],
            [0.0, -0.5, -1.0, -1.5, -1.0, -0.5, 0.0, 0.0, 0.0, 0.0],
        ),
    ];
    let tokens = eq_tokens(goal);
    let matched = RULES
        .iter()
        .filter(|(_, terms, _)| terms.iter().any(|term| tokens.contains(*term)))
        .collect::<Vec<_>>();
    let mut baseline = [0.0_f64; 10];
    for (_, _, gains) in &matched {
        for (value, gain) in baseline.iter_mut().zip(gains) {
            *value += gain;
        }
    }
    for value in &mut baseline {
        *value = ((*value).clamp(-4.0, 4.0) * 2.0).round() / 2.0;
    }
    EqLocalGuidance {
        method: "deterministic-eq-intent/v1",
        matched_rules: matched.iter().map(|(id, _, _)| (*id).to_owned()).collect(),
        bands: EQ_FREQUENCIES
            .iter()
            .zip(baseline)
            .map(|(frequency_hz, value)| EqBandGuidance {
                frequency_hz: *frequency_hz,
                baseline_gain_db: value,
                minimum_gain_db: (value - 1.5).clamp(-6.0, 0.0),
                maximum_gain_db: (value + 1.5).clamp(0.0, 4.0),
            })
            .collect(),
    }
}

fn eq_tokens(goal: &str) -> BTreeSet<String> {
    goal.to_lowercase()
        .split(|character: char| !(character.is_ascii_alphanumeric() || character == '-'))
        .filter(|token| !token.is_empty())
        .map(str::to_owned)
        .collect()
}

fn validate_output(output: &EqDraftOutput) -> Result<(), ModelTaskError> {
    if output.schema_version != EQ_DRAFT_OUTPUT_CONTRACT
        || output.gains_db.len() != EQ_FREQUENCIES.len()
        || output.rationale.is_empty()
        || output.rationale.chars().count() > 1_000
        || output.cautions.len() > 5
        || output
            .cautions
            .iter()
            .any(|value| value.is_empty() || value.chars().count() > 256)
        || output.gains_db.iter().any(|gain| {
            !gain.is_finite()
                || !(-12.0..=12.0).contains(gain)
                || (gain * 2.0 - (gain * 2.0).round()).abs() > 1e-9
        })
    {
        return Err(ModelTaskError::invalid_output("invalid EQ output fields"));
    }
    Ok(())
}

fn bound_incidental_text(mut payload: Value) -> Value {
    let Some(object) = payload.as_object_mut() else {
        return payload;
    };
    if let Some(Value::String(rationale)) = object.get_mut("rationale") {
        *rationale = truncate_chars(rationale, 1_000);
    }
    if let Some(Value::Array(cautions)) = object.get_mut("cautions") {
        cautions.truncate(5);
        for caution in cautions {
            if let Value::String(value) = caution {
                *value = truncate_chars(value, 256);
            }
        }
    }
    payload
}

fn eq_output_schema() -> Value {
    let mut schema = super::structured_harness::output_schema::<EqDraftOutput>();
    let properties = &mut schema["properties"];
    properties["schema_version"]["const"] = json!(EQ_DRAFT_OUTPUT_CONTRACT);
    properties["gains_db"]["minItems"] = json!(EQ_FREQUENCIES.len());
    properties["gains_db"]["maxItems"] = json!(EQ_FREQUENCIES.len());
    properties["gains_db"]["items"]["minimum"] = json!(-12.0);
    properties["gains_db"]["items"]["maximum"] = json!(12.0);
    properties["gains_db"]["items"]["multipleOf"] = json!(0.5);
    properties["rationale"]["minLength"] = json!(1);
    properties["rationale"]["maxLength"] = json!(1000);
    properties["cautions"]["maxItems"] = json!(5);
    properties["cautions"]["items"]["minLength"] = json!(1);
    properties["cautions"]["items"]["maxLength"] = json!(256);
    schema
}

fn compact_number(value: f64) -> String {
    if value.fract().abs() < f64::EPSILON {
        format!("{value:.0}")
    } else {
        value.to_string()
    }
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::{EQ_FREQUENCIES, EqDraftTask, build_local_eq_guidance, eq_quality_suite};
    use crate::assistant::StructuredModelResult;

    #[test]
    fn derived_schema_agrees_with_strict_eq_results() -> Result<(), Box<dyn std::error::Error>> {
        use crate::assistant::structured_harness::tests::{assert_output_contract, model_result};
        let task = EqDraftTask::new("Neutral", "neutral")?;
        let schema = task.request().output_schema.ok_or("missing schema")?;
        let valid = json!({"schema_version": super::EQ_DRAFT_OUTPUT_CONTRACT,
            "gains_db": vec![0.0; 10], "rationale": "Neutral baseline.", "cautions": ["Review first."]});
        assert_output_contract(&schema, &valid, |value| {
            task.finish(model_result(value)).is_ok()
        })?;
        for invalid_gain in [json!(0.25), json!(12.5), json!("0")] {
            let mut invalid = valid.clone();
            invalid["gains_db"][0] = invalid_gain;
            assert!(!jsonschema::is_valid(&schema, &invalid));
            assert!(task.finish(model_result(invalid)).is_err());
        }
        // Per-band envelopes are relational checks; the schema is the outer bound.
        let mut outside_envelope = valid.clone();
        outside_envelope["gains_db"][0] = json!(12.0);
        assert!(jsonschema::is_valid(&schema, &outside_envelope));
        assert!(task.finish(model_result(outside_envelope)).is_err());
        let mut verbose = valid;
        verbose["rationale"] = json!("a".repeat(1001));
        assert!(!jsonschema::is_valid(&schema, &verbose));
        assert!(task.finish(model_result(verbose)).is_ok());
        Ok(())
    }

    #[test]
    fn deterministic_guidance_is_bounded_and_composes_matching_rules() {
        let guidance = build_local_eq_guidance("warm wooden body without piercing harshness");
        assert_eq!(guidance.matched_rules, ["warmth", "reduce-harshness"]);
        assert_eq!(guidance.bands.len(), EQ_FREQUENCIES.len());
        assert!(guidance.bands.iter().all(|band| {
            band.minimum_gain_db <= band.baseline_gain_db
                && band.baseline_gain_db <= band.maximum_gain_db
        }));
    }

    #[test]
    fn output_is_strict_step_bounded_and_locally_enveloped()
    -> Result<(), Box<dyn std::error::Error>> {
        let task = EqDraftTask::new("Warm", "warm wooden tavern")?;
        let gains = build_local_eq_guidance("warm wooden tavern")
            .bands
            .into_iter()
            .map(|band| band.baseline_gain_db)
            .collect::<Vec<_>>();
        let draft = task.finish(StructuredModelResult {
            outcome: crate::assistant::ProviderAttemptOutcome::ResponseReceived,
            succeeded: true,
            error_code: None,
            payload: Some(json!({
                "schema_version": "assistant-eq-draft-output/v1",
                "gains_db": gains,
                "rationale": "A conservative warm curve.",
                "cautions": ["Listen at matched volume."]
            })),
            provider_model_id: None,
            finish_reason: Some("stop".to_owned()),
            input_tokens: None,
            output_tokens: None,
        })?;
        assert_eq!(draft.bands.len(), 10);
        Ok(())
    }

    #[test]
    fn bundled_quality_suite_is_current_and_complete() -> Result<(), Box<dyn std::error::Error>> {
        let suite = eq_quality_suite()?;
        assert_eq!(suite.cases.len(), 10);
        Ok(())
    }
}
