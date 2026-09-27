//! Native typed judgments, independent of chat generation and audio decoding.
use super::{
    ModelTaskError, ModelTransportFuture, ProviderExecutionTarget, StructuredModelRequest,
    StructuredModelResult,
};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::collections::BTreeMap;

pub(crate) const MAX_TYPED_QUESTIONS: usize = 256;
pub(crate) const MAX_TYPED_BODY_BYTES: usize = 64_000 - 1024;

pub const TYPED_DECISION_CONTRACT: &str = "typed-decisions/v1";
pub const TYPESAFE_ADAPTER: &str = "typesafe-systemone/v1";
pub const TYPED_DECISIONS_CAPABILITY: &str = "typed-decisions/v1";
pub const MOOD_DECISIONS_CAPABILITY: &str = "mood-decisions/v1";

#[derive(Debug, Clone, Eq, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "lowercase", deny_unknown_fields)]
pub enum TypedQuestion {
    Noul {
        instructions: Value,
        criteria: BTreeMap<String, Value>,
    },
    Choice {
        instructions: Value,
        criteria: BTreeMap<String, Value>,
    },
}

#[derive(Debug, Clone, Eq, PartialEq, Serialize)]
pub struct TypedDecisionRequest {
    pub state: Value,
    pub questions: BTreeMap<String, TypedQuestion>,
}

pub trait TypedDecisionTransport: std::fmt::Debug + Send + Sync {
    fn validate_typed_request(
        &self,
        target: &ProviderExecutionTarget,
        request: &TypedDecisionRequest,
    ) -> Result<(), ModelTaskError>;
    fn execute_typed_request<'a>(
        &'a self,
        target: &'a ProviderExecutionTarget,
        request: &'a TypedDecisionRequest,
    ) -> ModelTransportFuture<'a>;
}

impl TypedDecisionRequest {
    pub fn validate(&self) -> Result<(), ModelTaskError> {
        if self.questions.is_empty()
            || self.questions.len() > MAX_TYPED_QUESTIONS
            || !structured_value(&self.state)
        {
            return Err(ModelTaskError::new("invalid_request"));
        }
        let state_bytes = self.state.to_string().len();
        for (key, question) in &self.questions {
            let (instructions, criteria) = match question {
                TypedQuestion::Noul {
                    instructions,
                    criteria,
                } => {
                    if criteria.keys().map(String::as_str).collect::<Vec<_>>() != ["false", "true"]
                    {
                        return Err(ModelTaskError::new("invalid_request"));
                    }
                    if criteria.values().any(|value| !structured_value(value)) {
                        return Err(ModelTaskError::new("invalid_request"));
                    }
                    (instructions, criteria)
                }
                TypedQuestion::Choice {
                    instructions,
                    criteria,
                } => {
                    if !(2..=255).contains(&criteria.len()) {
                        return Err(ModelTaskError::new("invalid_request"));
                    }
                    if criteria
                        .values()
                        .any(|value| !value.is_null() && !structured_value(value))
                    {
                        return Err(ModelTaskError::new("invalid_request"));
                    }
                    (instructions, criteria)
                }
            };
            if key.is_empty()
                || key.len() > 128
                || !structured_value(instructions)
                || criteria.keys().any(|key| key.is_empty() || key.len() > 128)
            {
                return Err(ModelTaskError::new("invalid_request"));
            }
            // UTF-8 bytes are deliberately conservative reservation units, not token estimates.
            if state_bytes
                + serde_json::to_string(question)
                    .map_err(|_| ModelTaskError::new("invalid_request"))?
                    .len()
                + 1024
                > 32_000
            {
                return Err(ModelTaskError::new("request_too_large"));
            }
        }
        if serde_json::to_vec(self)
            .map_err(|_| ModelTaskError::new("invalid_request"))?
            .len()
            > MAX_TYPED_BODY_BYTES
        {
            return Err(ModelTaskError::new("request_too_large"));
        }
        Ok(())
    }

    /// Reuse the durable ledger, never the chat wire format. No generated explanation is requested.
    #[must_use]
    pub fn accounting_request(&self) -> StructuredModelRequest {
        StructuredModelRequest {
            system_prompt: TYPED_DECISION_CONTRACT.to_owned(),
            user_prompt: json!({"state":self.state,"questions":self.questions}).to_string(),
            max_output_tokens: 0,
            output_schema_name: None,
            output_schema: None,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Deserialize)]
#[serde(tag = "type", rename_all = "lowercase", deny_unknown_fields)]
pub enum TypedAnswer {
    Noul {
        noul: f64,
    },
    Choice {
        choice: String,
        probabilities: BTreeMap<String, f64>,
        confidence: f64,
    },
}

fn structured_value(value: &Value) -> bool {
    value.is_string() || value.is_object() || value.is_array()
}

fn probability(value: f64) -> bool {
    value.is_finite() && (0.0..=1.0).contains(&value)
}

pub fn typed_answers(
    request: &TypedDecisionRequest,
    payload: Value,
) -> Result<BTreeMap<String, TypedAnswer>, ModelTaskError> {
    let answers: BTreeMap<String, TypedAnswer> = serde_json::from_value(payload)
        .map_err(|_| ModelTaskError::new("typed_answer_shape_invalid"))?;
    if answers.keys().ne(request.questions.keys()) {
        return Err(ModelTaskError::new("typed_answer_set_mismatch"));
    }
    // TypeSafe documents approximate Choice probabilities, without a rounding bound.
    // Keep the reported scores: an invented sum tolerance or renormalization can
    // reject a usable answer or promote a candidate across its application gate.
    for (key, question) in &request.questions {
        let error = match (question, &answers[key]) {
            (TypedQuestion::Noul { .. }, TypedAnswer::Noul { noul }) => {
                (!probability(*noul)).then_some("typed_probability_invalid")
            }
            (
                TypedQuestion::Choice { criteria, .. },
                TypedAnswer::Choice {
                    choice,
                    probabilities,
                    confidence,
                },
            ) => {
                if !probability(*confidence)
                    || probabilities.values().any(|value| !probability(*value))
                {
                    Some("typed_probability_invalid")
                } else if criteria.keys().ne(probabilities.keys()) {
                    Some("typed_choice_options_mismatch")
                } else if !probabilities.get(choice).is_some_and(|selected| {
                    *selected > 0.0 && probabilities.values().all(|value| value <= selected)
                }) {
                    Some("typed_choice_selection_invalid")
                } else {
                    None
                }
            }
            _ => Some("typed_answer_type_mismatch"),
        };
        if let Some(code) = error {
            return Err(ModelTaskError::new(code));
        }
    }
    Ok(answers)
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct TypeSafeResponse {
    model: String,
    answers: Value,
    usage: TypeSafeUsage,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct TypeSafeUsage {
    input_tokens: u64,
    output_tokens: u64,
}

pub fn parse_typesafe_response(
    model: &str,
    request: &TypedDecisionRequest,
    value: Value,
) -> Result<StructuredModelResult, ModelTaskError> {
    let response: TypeSafeResponse = serde_json::from_value(value)
        .map_err(|_| ModelTaskError::new("typed_response_shape_invalid"))?;
    if response.model != model {
        return Err(ModelTaskError::new("provider_model_mismatch"));
    }
    typed_answers(request, response.answers.clone())?;
    Ok(StructuredModelResult {
        token_details: Default::default(),
        outcome: super::ProviderAttemptOutcome::ResponseReceived,
        succeeded: true,
        error_code: None,
        payload: Some(response.answers),
        provider_model_id: Some(response.model),
        finish_reason: None,
        input_tokens: Some(response.usage.input_tokens),
        output_tokens: Some(response.usage.output_tokens),
    })
}

/// Test direct semantic judgments; the nonce only correlates answers locally.
/// Jev does not receive question IDs, so it never has to compare random strings.
#[must_use]
pub fn typed_conformance_request(challenge: &str) -> TypedDecisionRequest {
    TypedDecisionRequest {
        state: json!({"recording_description":
            "A solo singer performs a melody using only the human voice. No musical instruments are played. There are no drums or percussion sounds."}),
        questions: BTreeMap::from([
            (
                format!("yes_{challenge}"),
                TypedQuestion::Noul {
                    instructions: json!("Does the described recording contain human singing?"),
                    criteria: BTreeMap::from([
                        ("true".to_owned(), json!("A human voice sings a melody.")),
                        ("false".to_owned(), json!("No human singing is present.")),
                    ]),
                },
            ),
            (
                format!("no_{challenge}"),
                TypedQuestion::Noul {
                    instructions: json!(
                        "Does the described recording contain drums or percussion?"
                    ),
                    criteria: BTreeMap::from([
                        (
                            "true".to_owned(),
                            json!("Drums or percussion instruments are audible."),
                        ),
                        (
                            "false".to_owned(),
                            json!("No drums or percussion instruments are audible."),
                        ),
                    ]),
                },
            ),
            (
                format!("choice_{challenge}"),
                TypedQuestion::Choice {
                    instructions: json!("Which description best matches the recording?"),
                    criteria: BTreeMap::from([
                        (
                            "solo_singing".to_owned(),
                            json!("A human voice sings without instrumental accompaniment."),
                        ),
                        (
                            "instrumental_music".to_owned(),
                            json!("Musical instruments play without any human singing."),
                        ),
                        ("silence".to_owned(), json!("There is no audible sound.")),
                    ]),
                },
            ),
        ]),
    }
}

pub fn validate_typed_conformance(
    challenge: &str,
    result: &StructuredModelResult,
) -> Result<(), ModelTaskError> {
    if !result.succeeded {
        return Err(ModelTaskError::new(
            result.error_code.as_deref().unwrap_or("invalid_response"),
        ));
    }
    let request = typed_conformance_request(challenge);
    let payload = result
        .payload
        .clone()
        .ok_or_else(|| ModelTaskError::new("invalid_typed_decisions"))?;
    let answers = typed_answers(&request, payload)?;
    if !matches!(answers.get(&format!("yes_{challenge}")), Some(TypedAnswer::Noul { noul }) if *noul >= 0.9)
    {
        return Err(ModelTaskError::new("typed_conformance_positive_failed"));
    }
    if !matches!(answers.get(&format!("no_{challenge}")), Some(TypedAnswer::Noul { noul }) if *noul <= 0.1)
    {
        return Err(ModelTaskError::new("typed_conformance_negative_failed"));
    }
    if !matches!(answers.get(&format!("choice_{challenge}")), Some(TypedAnswer::Choice { choice, probabilities, .. }) if choice == "solo_singing" && probabilities[choice] >= 0.9)
    {
        return Err(ModelTaskError::new("typed_conformance_choice_failed"));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn typed_conformance_keeps_random_identifiers_out_of_semantic_input()
    -> Result<(), Box<dyn std::error::Error>> {
        let challenge = "R7nYj8-Hd_4fK2spQ6wxZg";
        let request = typed_conformance_request(challenge);
        request.validate()?;
        assert!(!request.state.to_string().contains(challenge));
        for (id, question) in &request.questions {
            assert!(id.ends_with(challenge));
            assert!(!serde_json::to_string(question)?.contains(challenge));
        }
        Ok(())
    }

    fn response(challenge: &str) -> Value {
        json!({"model":"jev-1.13.0","answers":{
            format!("yes_{challenge}"):{"type":"noul","noul":0.98},
            format!("no_{challenge}"):{"type":"noul","noul":0.02},
            format!("choice_{challenge}"):{"type":"choice","choice":"solo_singing","probabilities":{"solo_singing":0.98,"instrumental_music":0.01,"silence":0.01},"confidence":0.9}},
            "usage":{"input_tokens":330,"output_tokens":20}})
    }
    #[test]
    fn typed_conformance_binds_all_answers_to_the_nonce_and_pinned_model()
    -> Result<(), Box<dyn std::error::Error>> {
        let request = typed_conformance_request("nonce-one");
        request.validate()?;
        let result = parse_typesafe_response("jev-1.13.0", &request, response("nonce-one"))?;
        validate_typed_conformance("nonce-one", &result)?;
        assert!(validate_typed_conformance("nonce-two", &result).is_err());
        assert_eq!(result.input_tokens, Some(330));
        assert_eq!(
            parse_typesafe_response("jev-1.12.0", &request, response("nonce-one"))
                .err()
                .ok_or("expected model mismatch")?
                .code,
            "provider_model_mismatch"
        );
        let mut inverted = response("nonce-one");
        inverted["answers"]["no_nonce-one"]["noul"] = json!(0.99);
        assert_eq!(
            validate_typed_conformance(
                "nonce-one",
                &parse_typesafe_response("jev-1.13.0", &request, inverted)?
            )
            .err()
            .ok_or("expected negative check failure")?
            .code,
            "typed_conformance_negative_failed"
        );
        Ok(())
    }
    #[test]
    fn typed_conformance_enforces_each_threshold_and_reports_the_failed_check()
    -> Result<(), Box<dyn std::error::Error>> {
        let challenge = "R7nYj8-Hd_4fK2spQ6wxZg";
        let request = typed_conformance_request(challenge);
        let target = super::super::ProviderConformanceTarget {
            role_id: "music_tagger".to_owned(),
            execution: super::super::ProviderExecutionTarget {
                adapter_id: super::super::TYPESAFE_ADAPTER.to_owned(),
                base_url: "https://api.typesafe.ai/v1".to_owned(),
                api_key: super::super::ProviderSecret::new("fixture-only"),
                allow_private_network: false,
                model_id: "jev-1.13.0".to_owned(),
                thinking_mode: super::super::ThinkingMode::ProviderDefault,
                timeout_seconds: 60,
                max_output_tokens: 2_000,
            },
            challenge: challenge.to_owned(),
            runtime_fingerprint: "a".repeat(64),
            role_configuration_fingerprint: "b".repeat(64),
            connection_fingerprint: "c".repeat(64),
        };
        let mut boundary = response(challenge);
        boundary["answers"][format!("yes_{challenge}")]["noul"] = json!(0.9);
        boundary["answers"][format!("no_{challenge}")]["noul"] = json!(0.1);
        boundary["answers"][format!("choice_{challenge}")]["probabilities"] =
            json!({"solo_singing":0.9,"instrumental_music":0.05,"silence":0.05});
        let result = parse_typesafe_response("jev-1.13.0", &request, boundary.clone())?;
        assert!(target.evaluate(result).passed);
        for (suffix, value, expected_error) in [
            (
                format!("yes_{challenge}/noul"),
                json!(0.8999),
                "typed_conformance_positive_failed",
            ),
            (
                format!("no_{challenge}/noul"),
                json!(0.1001),
                "typed_conformance_negative_failed",
            ),
            (
                format!("choice_{challenge}/probabilities"),
                json!({"solo_singing":0.8999,"instrumental_music":0.0501,"silence":0.05}),
                "typed_conformance_choice_failed",
            ),
            (
                format!("choice_{challenge}"),
                json!({"type":"choice","choice":"instrumental_music","probabilities":{"solo_singing":0.01,"instrumental_music":0.98,"silence":0.01},"confidence":0.9}),
                "typed_conformance_choice_failed",
            ),
        ] {
            let mut rejected = boundary.clone();
            *rejected
                .pointer_mut(&format!("/answers/{suffix}"))
                .ok_or("answer path")? = value;
            let result =
                target.evaluate(parse_typesafe_response("jev-1.13.0", &request, rejected)?);
            assert!(!result.passed);
            assert_eq!(result.error_code.as_deref(), Some(expected_error));
            assert_eq!(result.provider_model_id.as_deref(), Some("jev-1.13.0"));
            assert_eq!(result.input_tokens, Some(330));
        }
        let mut failed = parse_typesafe_response("jev-1.13.0", &request, boundary)?;
        failed.succeeded = false;
        failed.error_code = Some("timeout".to_owned());
        assert_eq!(
            validate_typed_conformance(challenge, &failed)
                .err()
                .ok_or("expected failure")?
                .code,
            "timeout"
        );
        assert_eq!(
            target.evaluate(failed).error_code.as_deref(),
            Some("timeout")
        );
        Ok(())
    }

    #[test]
    fn typed_choice_accepts_approximate_probabilities_without_normalizing()
    -> Result<(), Box<dyn std::error::Error>> {
        let request = typed_conformance_request("nonce");
        for scores in [
            json!({"solo_singing":0.98,"instrumental_music":0.01,"silence":0.0}),
            json!({"solo_singing":0.98,"instrumental_music":0.02,"silence":0.01}),
            json!({"solo_singing":0.33,"instrumental_music":0.33,"silence":0.33}),
            json!({"solo_singing":0.899,"instrumental_music":0.05,"silence":0.05}),
        ] {
            let mut raw = response("nonce");
            raw["answers"]["choice_nonce"]["probabilities"] = scores.clone();
            let result = parse_typesafe_response("jev-1.13.0", &request, raw)?;
            assert_eq!(
                result.payload.as_ref().ok_or("answers")?["choice_nonce"]["probabilities"],
                scores
            );
            // Rounding acceptance must not lower conformance's semantic threshold.
            assert_eq!(
                validate_typed_conformance("nonce", &result).is_ok(),
                scores["solo_singing"].as_f64().ok_or("score")? >= 0.9
            );
        }
        Ok(())
    }

    #[test]
    fn typed_answers_reject_missing_extra_wrong_types_and_invalid_scores()
    -> Result<(), Box<dyn std::error::Error>> {
        let request = typed_conformance_request("nonce");
        let valid = response("nonce")["answers"].clone();
        typed_answers(&request, valid.clone())?;
        for (path, value, code) in [
            ("/yes_nonce/noul", json!(-0.1), "typed_probability_invalid"),
            ("/yes_nonce/noul", json!(1.01), "typed_probability_invalid"),
            (
                "/yes_nonce/noul",
                json!("0.9"),
                "typed_answer_shape_invalid",
            ),
            (
                "/yes_nonce/type",
                json!("score"),
                "typed_answer_shape_invalid",
            ),
            (
                "/yes_nonce",
                json!({"type":"choice","choice":"a","probabilities":{"a":1.0,"b":0.0},"confidence":1.0}),
                "typed_answer_type_mismatch",
            ),
            (
                "/choice_nonce/confidence",
                json!(1.1),
                "typed_probability_invalid",
            ),
            (
                "/choice_nonce/probabilities/solo_singing",
                json!(-0.1),
                "typed_probability_invalid",
            ),
            (
                "/choice_nonce/choice",
                json!("missing"),
                "typed_choice_selection_invalid",
            ),
            (
                "/choice_nonce/choice",
                json!("instrumental_music"),
                "typed_choice_selection_invalid",
            ),
            (
                "/choice_nonce/probabilities",
                json!({"solo_singing":0.0,"instrumental_music":0.0,"silence":0.0}),
                "typed_choice_selection_invalid",
            ),
            (
                "/choice_nonce/probabilities",
                json!({"solo_singing":0.8,"injected":0.1,"silence":0.1}),
                "typed_choice_options_mismatch",
            ),
        ] {
            let mut invalid = valid.clone();
            *invalid.pointer_mut(path).ok_or("path")? = value;
            assert_eq!(
                typed_answers(&request, invalid)
                    .err()
                    .ok_or("expected invalid answer")?
                    .code,
                code,
                "{path}"
            );
        }
        let mut missing = valid.clone();
        missing.as_object_mut().ok_or("object")?.remove("yes_nonce");
        assert_eq!(
            typed_answers(&request, missing)
                .err()
                .ok_or("missing answer")?
                .code,
            "typed_answer_set_mismatch"
        );
        let mut extra = valid;
        extra["injected"] = json!({"type":"noul","noul":1.0});
        assert_eq!(
            typed_answers(&request, extra)
                .err()
                .ok_or("extra answer")?
                .code,
            "typed_answer_set_mismatch"
        );
        let mut invalid_envelope = response("nonce");
        invalid_envelope["injected"] = json!("unexpected");
        assert_eq!(
            parse_typesafe_response("jev-1.13.0", &request, invalid_envelope)
                .err()
                .ok_or("invalid envelope")?
                .code,
            "typed_response_shape_invalid"
        );
        Ok(())
    }
    #[test]
    fn typed_requests_enforce_both_context_limits_and_primitive_shapes()
    -> Result<(), Box<dyn std::error::Error>> {
        let mut request = typed_conformance_request("nonce");
        request.state = json!("a".repeat(31_500));
        assert_eq!(
            request
                .validate()
                .err()
                .ok_or("expected per-question limit")?
                .code,
            "request_too_large"
        );
        request.state = json!({"value":"small"});
        let question = TypedQuestion::Noul {
            instructions: json!("q".repeat(20_000)),
            criteria: BTreeMap::from([
                ("false".to_owned(), json!("no")),
                ("true".to_owned(), json!("yes")),
            ]),
        };
        request.questions = (0..4)
            .map(|index| (format!("q{index}"), question.clone()))
            .collect();
        assert_eq!(
            request
                .validate()
                .err()
                .ok_or("expected total context limit")?
                .code,
            "request_too_large"
        );
        request.questions.remove("q3");
        request.validate()?;
        assert_eq!(request.accounting_request().max_output_tokens, 0);
        request.state = json!(42);
        assert!(request.validate().is_err());
        let mut request = typed_conformance_request("nonce");
        if let Some(TypedQuestion::Choice { criteria, .. }) =
            request.questions.get_mut("choice_nonce")
        {
            criteria.insert("bad".to_owned(), json!(42));
        }
        assert!(request.validate().is_err());
        Ok(())
    }
}
