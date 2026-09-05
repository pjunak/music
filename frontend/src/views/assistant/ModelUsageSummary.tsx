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

  const unreportedRequests =
    usage.outcomes?.responses_missing_usage ??
    Math.max(
      usage.attempted_requests - usage.input_tokens_reported_requests,
      usage.attempted_requests - usage.output_tokens_reported_requests,
    );
  const requestLabel = usage.outcomes
    ? (usage.attempted_requests === 1 ? "attempt" : "attempts")
    : (usage.attempted_requests === 1 ? "model call" : "model calls");

  return (
    <aside className="assistant-model-usage" aria-label="Recorded provider usage">
      <div className="assistant-model-usage-heading">
        <strong>Recorded provider usage</strong>
        <span>
          {formatCount(usage.attempted_requests)} {requestLabel}
        </span>
      </div>
      {usage.outcomes ? (
        <>
          <p>
            {formatCount(usage.outcomes.response_received)} responses received ·{" "}
            {formatCount(usage.outcomes.not_sent)} not sent ·{" "}
            {formatCount(usage.outcomes.preflight_rejected)} rejected before sending
          </p>
          {usage.outcomes.uncertain > 0 ? (
            <p className="assistant-model-usage-warning">
              {formatCount(usage.outcomes.uncertain)}{" "}
              {usage.outcomes.uncertain === 1 ? "attempt has" : "attempts have"}{" "}
              {job?.status === "running" || job?.status === "cancel_requested"
                ? "an in-progress or uncertain outcome."
                : "an uncertain outcome."}{" "}
              Token totals may be incomplete.
            </p>
          ) : null}
          <p>
            Queue wait:{" "}
            {usage.outcomes.queue_wait_seconds === null
              ? "unavailable"
              : `~${formatCount(usage.outcomes.queue_wait_seconds)} s`} ·{" "}
            Completed requests:{" "}
            {formatCount(usage.outcomes.completed_attempt_elapsed_ms)} ms ·{" "}
            Request limit: {formatCount(usage.outcomes.max_attempts)}
          </p>
        </>
      ) : null}
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
          {usage.outcomes
            ? `Token counts are incomplete for ${unreportedRequests} of ${usage.outcomes.response_received} responses.`
            : `The provider omitted one or both token counts for at least ${unreportedRequests} of ${usage.attempted_requests} calls.`}{" "}
          Totals include only reported usage.
        </p>
      ) : null}
      <small>Exact charges depend on your provider and are not estimated here.</small>
    </aside>
  );
}
