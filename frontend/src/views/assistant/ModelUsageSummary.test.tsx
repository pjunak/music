import { render, screen } from "@testing-library/react";
import { describe, expect, it } from "vitest";

import type { BackgroundJob } from "@/core/api";

import { providerUsageFromJob } from "./modelUsage";
import { ModelUsageSummary } from "./ModelUsageSummary";

function completedJob(usage: Record<string, unknown>): BackgroundJob {
  return {
    id: "usage-job",
    kind: "assistant.model-evaluation.playlist-quality-v1",
    status: "succeeded",
    parameters: {},
    result: { usage },
    error: null,
    progress_current: 1,
    progress_total: 1,
    progress_phase: "Complete",
    progress_message: "",
    attempts: 1,
    retry_of_id: null,
    created_at: "2026-08-19T12:00:00Z",
    updated_at: "2026-08-19T12:00:01Z",
    started_at: "2026-08-19T12:00:00Z",
    finished_at: "2026-08-19T12:00:01Z",
  };
}

const completeUsage = {
  schema_version: "assistant-provider-usage/v1",
  attempted_requests: 2,
  input_tokens: 1240,
  output_tokens: 318,
  input_tokens_reported_requests: 2,
  output_tokens_reported_requests: 2,
  provider_model_ids: ["planner-v2"],
  provider_model_ids_truncated: false,
};

describe("provider usage summary", () => {
  it("renders persisted requests, tokens, and the provider-reported model", () => {
    render(<ModelUsageSummary job={completedJob(completeUsage)} />);

    expect(screen.getByLabelText("Recorded provider usage")).toHaveTextContent(
      "2 model calls",
    );
    expect(screen.getByText(/input tokens$/)).toHaveTextContent(
      /1,?240 input tokens/,
    );
    expect(screen.getByText(/output tokens$/)).toHaveTextContent(
      "318 output tokens",
    );
    expect(screen.getByText("Reported model: planner-v2")).toBeInTheDocument();
  });

  it("warns when the provider omits usage instead of presenting zeros as exact", () => {
    render(
      <ModelUsageSummary
        job={completedJob({
          ...completeUsage,
          input_tokens_reported_requests: 1,
          output_tokens_reported_requests: 0,
        })}
      />,
    );

    expect(
      screen.getByText(/provider omitted one or both token counts for at least 2 of 2 calls/i),
    ).toBeInTheDocument();
  });

  it("rejects malformed or internally inconsistent stored usage", () => {
    expect(
      providerUsageFromJob(
        completedJob({
          ...completeUsage,
          input_tokens_reported_requests: 3,
        }),
      ),
    ).toBeNull();
    expect(
      providerUsageFromJob(
        completedJob({
          ...completeUsage,
          provider_model_ids: ["planner-v2", "planner-v2"],
        }),
      ),
    ).toBeNull();
  });
});

const measuredUsage = {
  ...completeUsage,
  schema_version: "assistant-provider-usage/v2",
  attempted_requests: 5,
  input_tokens_reported_requests: 1,
  output_tokens_reported_requests: 1,
  preflight_rejected_requests: 1,
  not_sent_requests: 1,
  response_received_requests: 2,
  uncertain_requests: 1,
  responses_missing_usage: 2,
  completed_attempt_elapsed_ms: 3450,
  run_manifest: {
    schema_version: "assistant-model-run/v1",
    max_attempts: 5,
    queue_wait_seconds: 12,
  },
};

describe("recorded provider outcomes", () => {
  it("separates unsent and uncertain attempts from incomplete responses", () => {
    render(<ModelUsageSummary job={completedJob(measuredUsage)} />);
    expect(screen.getByLabelText("Recorded provider usage")).toHaveTextContent("5 attempts");
    expect(screen.getByText(/2 responses received/)).toHaveTextContent("1 not sent · 1 rejected before sending");
    expect(screen.getByText(/1 attempt has an uncertain outcome/)).toBeInTheDocument();
    expect(screen.getByText(/incomplete for 2 of 2 responses/)).toBeInTheDocument();
    expect(screen.getByText(/Queue wait: ~12 s/)).toHaveTextContent(/Completed requests: 3,?450 ms/);
  });

  it("does not claim a running request is a lost response or missing provider usage", () => {
    const job = completedJob({
      ...measuredUsage, attempted_requests: 1,
      input_tokens: 0, output_tokens: 0,
      input_tokens_reported_requests: 0, output_tokens_reported_requests: 0,
      preflight_rejected_requests: 0, not_sent_requests: 0,
      response_received_requests: 0, responses_missing_usage: 0,
    });
    job.status = "running";
    render(<ModelUsageSummary job={job} />);
    expect(screen.getByText(/in-progress or uncertain outcome/)).toBeInTheDocument();
    expect(screen.queryByText(/counts are incomplete for/)).not.toBeInTheDocument();
  });

  it("rejects invalid outcome totals, budgets, timing, and unsafe counters", () => {
    for (const invalid of [
      { uncertain_requests: 2 }, { responses_missing_usage: 3 },
      { attempted_requests: Number.MAX_SAFE_INTEGER + 1 },
      { run_manifest: { ...measuredUsage.run_manifest, max_attempts: 4 } },
      { run_manifest: { ...measuredUsage.run_manifest, queue_wait_seconds: -1 } },
    ]) {
      expect(providerUsageFromJob(completedJob({ ...measuredUsage, ...invalid }))).toBeNull();
    }
  });
});
