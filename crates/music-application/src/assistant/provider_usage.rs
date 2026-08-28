use serde::Serialize;
use serde_json::{Map, Value, json};

use super::StructuredModelResult;

const MAX_PROVIDER_MODEL_IDS: usize = 8;

#[derive(Debug, Clone, Default, Eq, PartialEq, Serialize)]
pub struct ProviderUsageSummary {
    pub schema_version: &'static str,
    pub attempted_requests: u64,
    pub input_tokens: u64,
    pub output_tokens: u64,
    pub input_tokens_reported_requests: u64,
    pub output_tokens_reported_requests: u64,
    pub provider_model_ids: Vec<String>,
    pub provider_model_ids_truncated: bool,
}

#[derive(Debug, Default)]
pub struct ProviderUsageAccumulator {
    attempted_requests: u64,
    input_tokens: u64,
    output_tokens: u64,
    input_tokens_reported_requests: u64,
    output_tokens_reported_requests: u64,
    provider_model_ids: Vec<String>,
    provider_model_ids_truncated: bool,
}

impl ProviderUsageAccumulator {
    pub fn record(&mut self, result: &StructuredModelResult) {
        self.attempted_requests = self.attempted_requests.saturating_add(1);
        if let Some(input_tokens) = result.input_tokens {
            self.input_tokens = self.input_tokens.saturating_add(input_tokens);
            self.input_tokens_reported_requests =
                self.input_tokens_reported_requests.saturating_add(1);
        }
        if let Some(output_tokens) = result.output_tokens {
            self.output_tokens = self.output_tokens.saturating_add(output_tokens);
            self.output_tokens_reported_requests =
                self.output_tokens_reported_requests.saturating_add(1);
        }
        let Some(model_id) = result
            .provider_model_id
            .as_ref()
            .filter(|model_id| !model_id.is_empty() && model_id.chars().count() <= 256)
        else {
            return;
        };
        if self.provider_model_ids.contains(model_id) {
            return;
        }
        if self.provider_model_ids.len() < MAX_PROVIDER_MODEL_IDS {
            self.provider_model_ids.push(model_id.clone());
        } else {
            self.provider_model_ids_truncated = true;
        }
    }

    #[must_use]
    pub fn summary(&self) -> ProviderUsageSummary {
        ProviderUsageSummary {
            schema_version: "assistant-provider-usage/v1",
            attempted_requests: self.attempted_requests,
            input_tokens: self.input_tokens,
            output_tokens: self.output_tokens,
            input_tokens_reported_requests: self.input_tokens_reported_requests,
            output_tokens_reported_requests: self.output_tokens_reported_requests,
            provider_model_ids: self.provider_model_ids.clone(),
            provider_model_ids_truncated: self.provider_model_ids_truncated,
        }
    }

    #[must_use]
    pub fn checkpoint(&self) -> Map<String, Value> {
        json!({
            "schema_version": "assistant-provider-usage-checkpoint/v1",
            "usage": self.summary(),
        })
        .as_object()
        .cloned()
        .unwrap_or_default()
    }
}

#[cfg(test)]
mod tests {
    use super::ProviderUsageAccumulator;
    use crate::assistant::StructuredModelResult;

    #[test]
    fn usage_counts_only_reported_tokens_and_bounds_model_ids() {
        let mut usage = ProviderUsageAccumulator::default();
        for index in 0..10 {
            usage.record(&StructuredModelResult {
                succeeded: true,
                error_code: None,
                payload: None,
                provider_model_id: Some(format!("model-{index}")),
                finish_reason: None,
                input_tokens: (index == 0).then_some(10),
                output_tokens: (index == 1).then_some(5),
            });
        }
        let summary = usage.summary();
        assert_eq!(summary.attempted_requests, 10);
        assert_eq!(summary.input_tokens, 10);
        assert_eq!(summary.input_tokens_reported_requests, 1);
        assert_eq!(summary.output_tokens, 5);
        assert_eq!(summary.output_tokens_reported_requests, 1);
        assert_eq!(summary.provider_model_ids.len(), 8);
        assert!(summary.provider_model_ids_truncated);
    }
}
