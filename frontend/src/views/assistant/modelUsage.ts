import type { BackgroundJob } from "@/core/api";

export interface ProviderUsageSummary {
  schema_version: "assistant-provider-usage/v1" | "assistant-provider-usage/v2";
  attempted_requests: number;
  input_tokens: number;
  output_tokens: number;
  input_tokens_reported_requests: number;
  output_tokens_reported_requests: number;
  provider_model_ids: string[];
  provider_model_ids_truncated: boolean;
  outcomes?: {
    preflight_rejected: number;
    not_sent: number;
    response_received: number;
    uncertain: number;
    responses_missing_usage: number;
    completed_attempt_elapsed_ms: number;
    queue_wait_seconds: number | null;
    max_attempts: number;
  };
}

function isRecord(value: unknown): value is Record<string, unknown> {
  return typeof value === "object" && value !== null && !Array.isArray(value);
}

function isCount(value: unknown): value is number {
  return typeof value === "number" && Number.isSafeInteger(value) && value >= 0;
}

export function providerUsageFromJob(
  job: BackgroundJob | null | undefined,
): ProviderUsageSummary | null {
  const usage = job?.result?.usage;
  if (
    !isRecord(usage) ||
    (usage.schema_version !== "assistant-provider-usage/v1" &&
      usage.schema_version !== "assistant-provider-usage/v2") ||
    !isCount(usage.attempted_requests) ||
    !isCount(usage.input_tokens) ||
    !isCount(usage.output_tokens) ||
    !isCount(usage.input_tokens_reported_requests) ||
    !isCount(usage.output_tokens_reported_requests) ||
    usage.input_tokens_reported_requests > usage.attempted_requests ||
    usage.output_tokens_reported_requests > usage.attempted_requests ||
    !Array.isArray(usage.provider_model_ids) ||
    usage.provider_model_ids.length > 8 ||
    !usage.provider_model_ids.every(
      (modelId) =>
        typeof modelId === "string" && modelId.length > 0 && modelId.length <= 256,
    ) ||
    new Set(usage.provider_model_ids).size !== usage.provider_model_ids.length ||
    typeof usage.provider_model_ids_truncated !== "boolean"
  ) {
    return null;
  }
  let outcomes: ProviderUsageSummary["outcomes"];
  if (usage.schema_version === "assistant-provider-usage/v2") {
    const manifest = usage.run_manifest;
    if (
      !isCount(usage.preflight_rejected_requests) ||
      !isCount(usage.not_sent_requests) ||
      !isCount(usage.response_received_requests) ||
      !isCount(usage.uncertain_requests) ||
      !isCount(usage.responses_missing_usage) ||
      !isCount(usage.completed_attempt_elapsed_ms) ||
      usage.preflight_rejected_requests +
        usage.not_sent_requests +
        usage.response_received_requests +
        usage.uncertain_requests !== usage.attempted_requests ||
      usage.responses_missing_usage > usage.response_received_requests ||
      !isRecord(manifest) ||
      manifest.schema_version !== "assistant-model-run/v1" ||
      !isCount(manifest.max_attempts) ||
      manifest.max_attempts < usage.attempted_requests ||
      (manifest.queue_wait_seconds !== null && !isCount(manifest.queue_wait_seconds))
    ) {
      return null;
    }
    outcomes = {
      preflight_rejected: usage.preflight_rejected_requests,
      not_sent: usage.not_sent_requests,
      response_received: usage.response_received_requests,
      uncertain: usage.uncertain_requests,
      responses_missing_usage: usage.responses_missing_usage,
      completed_attempt_elapsed_ms: usage.completed_attempt_elapsed_ms,
      queue_wait_seconds: manifest.queue_wait_seconds,
      max_attempts: manifest.max_attempts,
    };
  }
  return {
    schema_version: usage.schema_version,
    attempted_requests: usage.attempted_requests,
    input_tokens: usage.input_tokens,
    output_tokens: usage.output_tokens,
    input_tokens_reported_requests: usage.input_tokens_reported_requests,
    output_tokens_reported_requests: usage.output_tokens_reported_requests,
    provider_model_ids: [...usage.provider_model_ids],
    provider_model_ids_truncated: usage.provider_model_ids_truncated,
    ...(outcomes ? { outcomes } : {}),
  };
}
