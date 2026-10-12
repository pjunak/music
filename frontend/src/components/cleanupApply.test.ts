import { beforeEach, describe, expect, it, vi } from "vitest";

import { assistantApi, cleanupApi } from "@/core/api";
import type { CleanupOpIn } from "@/core/api";

import { applyCleanupSelection } from "./cleanupApply";

vi.mock("@/core/api", () => ({
  assistantApi: { reviewAnalysisTagsBulk: vi.fn() },
  cleanupApi: { apply: vi.fn() },
}));

const op = (track_id: number): CleanupOpIn => ({
  track_id, kind: "tag", field: "title", old: "a", new: "b",
});
const tag = { track_id: 1, tag: "calm", analyzer_id: "catalog-tags/v1", source_signature: "s" };

beforeEach(() => {
  vi.clearAllMocks();
});

describe("applyCleanupSelection", () => {
  it("accepts tags before applying file changes in one batch", async () => {
    const calls: string[] = [];
    vi.mocked(assistantApi.reviewAnalysisTagsBulk).mockImplementation(async () => {
      calls.push("tags");
      return { requested_items: 1, applied: [], failures: [] };
    });
    vi.mocked(cleanupApi.apply).mockImplementation(async (_ops, batchId) => {
      calls.push(`apply:${batchId}`);
      return { applied: 20, skipped: [], batch_id: 7 };
    });
    const progress: number[] = [];
    const summary = await applyCleanupSelection(
      Array.from({ length: 25 }, (_, i) => op(i + 1)), [tag], "entire library",
      (done) => progress.push(done),
    );
    expect(calls).toEqual(["tags", "apply:null", "apply:7"]);
    expect(summary).toMatchObject({ applied: 40, batchId: 7, applyError: null, tagError: null });
    expect(progress).toEqual([1, 21, 26]);
  });

  it("stops a phase at its first failure and keeps what landed", async () => {
    vi.mocked(assistantApi.reviewAnalysisTagsBulk).mockRejectedValue(new Error("review down"));
    vi.mocked(cleanupApi.apply)
      .mockResolvedValueOnce({ applied: 20, skipped: [{ track_id: 3, reason: "missing" }], batch_id: 9 })
      .mockRejectedValueOnce(new Error("disk full"));
    const summary = await applyCleanupSelection(
      Array.from({ length: 30 }, (_, i) => op(i + 1)), [tag], "folder", () => {},
    );
    expect(summary).toMatchObject({
      applied: 20, batchId: 9, tagFailures: 1, tagError: "review down", applyError: "disk full",
    });
    expect(summary.skipped).toEqual([{ track_id: 3, reason: "missing" }]);
  });
});
