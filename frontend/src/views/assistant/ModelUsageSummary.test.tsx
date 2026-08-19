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
      screen.getByText(/provider omitted one or both token counts for 2 of 2 calls/i),
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
