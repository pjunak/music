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
        .map_err(|_| ModelTaskError::new("invalid_typed_decisions"))?;
    if answers.keys().ne(request.questions.keys()) {
        return Err(ModelTaskError::new("invalid_typed_decisions"));
    }
    for (key, question) in &request.questions {
        let valid = match (question, &answers[key]) {
            (TypedQuestion::Noul { .. }, TypedAnswer::Noul { noul }) => probability(*noul),
            (
                TypedQuestion::Choice { criteria, .. },
                TypedAnswer::Choice {
                    choice,
                    probabilities,
                    confidence,
                },
            ) => {
                probability(*confidence)
                    && criteria.keys().eq(probabilities.keys())
                    && probabilities.values().all(|value| probability(*value))
                    && (probabilities.values().sum::<f64>() - 1.0).abs() <= 0.0001
                    && probabilities.get(choice).is_some_and(|selected| {
                        probabilities.values().all(|value| value <= selected)
                    })
            }
            _ => false,
        };
        if !valid {
            return Err(ModelTaskError::new("invalid_typed_decisions"));
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
        .map_err(|_| ModelTaskError::new("invalid_typed_decisions"))?;
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

/// The nonce occurs in the question IDs and in a closed choice, so an old response cannot pass.
#[must_use]
pub fn typed_conformance_request(challenge: &str) -> TypedDecisionRequest {
    TypedDecisionRequest {
        state: json!({"reference":challenge,"supplied":challenge,"different":"not-the-reference"}),
        questions: BTreeMap::from([
            (
                format!("yes_{challenge}"),
                TypedQuestion::Noul {
                    instructions: json!("Does supplied exactly match reference?"),
                    criteria: BTreeMap::from([
                        ("true".to_owned(), json!("They match exactly.")),
                        ("false".to_owned(), json!("They differ.")),
                    ]),
                },
            ),
            (
                format!("no_{challenge}"),
                TypedQuestion::Noul {
                    instructions: json!("Does different exactly match reference?"),
                    criteria: BTreeMap::from([
                        ("true".to_owned(), json!("They match exactly.")),
                        ("false".to_owned(), json!("They differ.")),
                    ]),
                },
            ),
            (
                format!("choice_{challenge}"),
                TypedQuestion::Choice {
                    instructions: json!("Select the option identical to the state's reference."),
                    criteria: BTreeMap::from([
                        (challenge.to_owned(), Value::Null),
                        ("not-the-reference".to_owned(), Value::Null),
                    ]),
                },
            ),
        ]),
    }
}

pub fn typed_conformance_passed(challenge: &str, result: &StructuredModelResult) -> bool {
    let request = typed_conformance_request(challenge);
    if !result.succeeded {
        return false;
    }
    let Some(payload) = result.payload.clone() else {
        return false;
    };
    let Ok(answers) = typed_answers(&request, payload) else {
        return false;
    };
    matches!(answers.get(&format!("yes_{challenge}")),Some(TypedAnswer::Noul { noul }) if *noul >= 0.9)
        && matches!(answers.get(&format!("no_{challenge}")),Some(TypedAnswer::Noul { noul }) if *noul <= 0.1)
        && matches!(answers.get(&format!("choice_{challenge}")),Some(TypedAnswer::Choice { choice, probabilities, .. }) if choice == challenge && probabilities[choice] >= 0.9)
}

#[cfg(test)]
mod tests {
    use super::*;
    fn response(challenge: &str) -> Value {
        json!({"model":"jev-1.13.0","answers":{
            format!("yes_{challenge}"):{"type":"noul","noul":0.98},
            format!("no_{challenge}"):{"type":"noul","noul":0.02},
            format!("choice_{challenge}"):{"type":"choice","choice":challenge,"probabilities":{challenge:0.98,"not-the-reference":0.02},"confidence":0.9}},
            "usage":{"input_tokens":330,"output_tokens":20}})
    }
    #[test]
    fn typed_conformance_binds_all_answers_to_the_nonce_and_pinned_model()
    -> Result<(), Box<dyn std::error::Error>> {
        let request = typed_conformance_request("nonce-one");
        request.validate()?;
        let result = parse_typesafe_response("jev-1.13.0", &request, response("nonce-one"))?;
        assert!(typed_conformance_passed("nonce-one", &result));
        assert!(!typed_conformance_passed("nonce-two", &result));
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
        assert!(!typed_conformance_passed(
            "nonce-one",
            &parse_typesafe_response("jev-1.13.0", &request, inverted)?
        ));
        Ok(())
    }
    #[test]
    fn typed_answers_reject_missing_extra_wrong_types_and_invalid_distributions()
    -> Result<(), Box<dyn std::error::Error>> {
        let request = typed_conformance_request("nonce");
        let valid = response("nonce")["answers"].clone();
        typed_answers(&request, valid.clone())?;
        for (path, value) in [
            ("/yes_nonce/noul", json!(-0.1)),
            ("/yes_nonce/noul", json!(1.01)),
            ("/yes_nonce/noul", json!("0.9")),
            ("/yes_nonce/type", json!("score")),
            ("/choice_nonce/confidence", json!(1.1)),
            ("/choice_nonce/choice", json!("missing")),
            ("/choice_nonce/choice", json!("not-the-reference")),
            (
                "/choice_nonce/probabilities",
                json!({"nonce":0.8,"not-the-reference":0.1}),
            ),
            (
                "/choice_nonce/probabilities",
                json!({"nonce":0.8,"injected":0.2}),
            ),
        ] {
            let mut invalid = valid.clone();
            *invalid.pointer_mut(path).ok_or("path")? = value;
            assert!(typed_answers(&request, invalid).is_err(), "{path}");
        }
        let mut missing = valid.clone();
        missing.as_object_mut().ok_or("object")?.remove("yes_nonce");
        assert!(typed_answers(&request, missing).is_err());
        let mut extra = valid;
        extra["injected"] = json!({"type":"noul","noul":1.0});
        assert!(typed_answers(&request, extra).is_err());
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
