import type { BackgroundJob } from "@/core/api";

import { providerUsageFromJob } from "./modelUsage";

function formatCount(value: number): string {
  return new Intl.NumberFormat().format(value);
}

interface Props {
  job: BackgroundJob | null | undefined;
}

export function ModelUsageSummary({ job }: Props) {
  const usage = providerUsageFromJob(job);
  if (usage === null) return null;

  const unreportedRequests = Math.max(
    usage.attempted_requests - usage.input_tokens_reported_requests,
    usage.attempted_requests - usage.output_tokens_reported_requests,
  );
  const requestLabel = usage.attempted_requests === 1 ? "model call" : "model calls";

  return (
    <aside className="assistant-model-usage" aria-label="Recorded provider usage">
      <div className="assistant-model-usage-heading">
        <strong>Recorded provider usage</strong>
        <span>
          {formatCount(usage.attempted_requests)} {requestLabel}
        </span>
      </div>
      <div className="assistant-model-usage-tokens">
        <span>
          <strong>{formatCount(usage.input_tokens)}</strong> input tokens
        </span>
        <span>
          <strong>{formatCount(usage.output_tokens)}</strong> output tokens
        </span>
      </div>
      {usage.provider_model_ids.length > 0 ? (
        <p>
          Reported model: {usage.provider_model_ids.join(", ")}
          {usage.provider_model_ids_truncated ? " and additional model IDs" : ""}
        </p>
      ) : null}
      {unreportedRequests > 0 ? (
        <p className="assistant-model-usage-warning">
          The provider omitted one or both token counts for {unreportedRequests} of{" "}
          {usage.attempted_requests} calls. Totals include only reported usage.
        </p>
      ) : null}
      <small>Exact charges depend on your provider and are not estimated here.</small>
    </aside>
  );
}
