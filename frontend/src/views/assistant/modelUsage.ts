import type { BackgroundJob } from "@/core/api";

export interface ProviderUsageSummary {
  schema_version: "assistant-provider-usage/v1";
  attempted_requests: number;
  input_tokens: number;
  output_tokens: number;
  input_tokens_reported_requests: number;
  output_tokens_reported_requests: number;
  provider_model_ids: string[];
  provider_model_ids_truncated: boolean;
}

function isRecord(value: unknown): value is Record<string, unknown> {
  return typeof value === "object" && value !== null && !Array.isArray(value);
}

function isCount(value: unknown): value is number {
  return typeof value === "number" && Number.isInteger(value) && value >= 0;
}

export function providerUsageFromJob(
  job: BackgroundJob | null | undefined,
): ProviderUsageSummary | null {
  const usage = job?.result?.usage;
  if (
    !isRecord(usage) ||
    usage.schema_version !== "assistant-provider-usage/v1" ||
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
  return {
    schema_version: usage.schema_version,
    attempted_requests: usage.attempted_requests,
    input_tokens: usage.input_tokens,
    output_tokens: usage.output_tokens,
    input_tokens_reported_requests: usage.input_tokens_reported_requests,
    output_tokens_reported_requests: usage.output_tokens_reported_requests,
    provider_model_ids: [...usage.provider_model_ids],
    provider_model_ids_truncated: usage.provider_model_ids_truncated,
  };
}
