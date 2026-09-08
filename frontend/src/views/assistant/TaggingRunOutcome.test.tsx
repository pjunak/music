import { render, screen } from "@testing-library/react";
import { describe, expect, it } from "vitest";
import type { BackgroundJob } from "@/core/api";
import { TaggingRunOutcome } from "./TaggingRunOutcome";
import { taggingYield } from "./taggingYield";

function job(result: Record<string, unknown>): BackgroundJob {
  return { id: "run", kind: "assistant.model-music-tagging", status: "succeeded", result, parameters: {}, error: null, progress_current: 20, progress_total: 50, progress_phase: "", progress_message: "", attempts: 1, retry_of_id: null, created_at: "", updated_at: "", started_at: null, finished_at: null };
}
const legacy = { schema_version: "assistant-model-music-tagging-job-result/v6", analyzer_id: "model-context-tagger/v6", vocabulary_fingerprint: "a".repeat(64), library_tracks: 50, scope_tracks: 50, updated_profiles: 20, unchanged_profiles: 0, skipped_changed_tracks: 0, context_policy: "include", skipped_context_tracks: 0 };
const yieldResult = { processed_tracks: 20, tracks_with_suggestions: 0, tracks_without_suggestions: 20, suggested_tags: 0, updated_profiles: 20 };

describe("tagging outcomes", () => {
  it("explains a no-tag stop without describing empty results as suggestions", () => {
    render(<TaggingRunOutcome job={job({ ...legacy, schema_version: "assistant-model-music-tagging-job-result/v7", analyzer_id: "model-context-tagger/v7", ...yieldResult, deferred_tracks: 15, remaining_tracks: 15, stopped_empty_batch: true })} />);
    expect(screen.getByText("Stopped after a request returned no tags")).toBeInTheDocument();
    expect(screen.getByText(/20 tracks analysed: 0 with suggestions, 20 with no supported tags/)).toBeInTheDocument();
    expect(screen.getByText(/20 profiles saved; 0 already current; 15 deferred/)).toBeInTheDocument();
    expect(screen.getByText(/15 planned tracks were not sent/)).toBeInTheDocument();
    expect(screen.queryByText("Tag suggestions are ready for review")).not.toBeInTheDocument();
  });
  it("keeps the old analyzer readable without inventing yield counts", () => {
    render(<TaggingRunOutcome job={job(legacy)} />);
    expect(screen.getByText(/older run did not record separate counts/)).toBeInTheDocument();
    expect(screen.queryByText(/0 with suggestions/)).not.toBeInTheDocument();
  });
  it("reads partial counters retained in a later provider checkpoint", () => {
    render(<TaggingRunOutcome job={{ ...job({ schema_version: "assistant-provider-usage-checkpoint/v2", feature_progress: yieldResult }), status: "cancelled" }} />);
    expect(screen.getByText("Completed work retained")).toBeInTheDocument();
    expect(screen.getByText(/20 profiles saved before the run stopped/)).toBeInTheDocument();
  });
  it("rejects contradictory or invalid counts", () => {
    expect(taggingYield({ ...yieldResult, tracks_without_suggestions: 19 })).toBeNull();
    expect(taggingYield({ ...yieldResult, updated_profiles: 21 })).toBeNull();
    expect(taggingYield({ ...yieldResult, suggested_tags: -1 })).toBeNull();
  });
});
