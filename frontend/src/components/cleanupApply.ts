import { assistantApi, cleanupApi } from "@/core/api";
import type { AnalysisTagReviewTarget, CleanupOpIn } from "@/core/api";

const APPLY_CHUNK = 20;
const REVIEW_CHUNK = 500;

export interface CleanupApplySummary {
  applied: number;
  skipped: { track_id: number; reason: string }[];
  batchId: number | null;
  acceptedTags: number;
  tagFailures: number;
  /** Why tag acceptance stopped early, if it did. */
  tagError: string | null;
  /** Why file and metadata changes stopped early, if they did. */
  applyError: string | null;
}

/**
 * Accept the chosen catalog mood tags, then apply file/metadata/folder changes
 * in journaled chunks under one batch. Tags go first: their review targets are
 * bound to the metadata signature an embedded-tag repair would change.
 * A failed chunk stops that phase; everything before it has landed.
 */
export async function applyCleanupSelection(
  ops: CleanupOpIn[],
  tagTargets: AnalysisTagReviewTarget[],
  scopeLabel: string,
  onProgress: (done: number, total: number) => void,
): Promise<CleanupApplySummary> {
  const total = ops.length + tagTargets.length;
  const summary: CleanupApplySummary = {
    applied: 0,
    skipped: [],
    batchId: null,
    acceptedTags: 0,
    tagFailures: 0,
    tagError: null,
    applyError: null,
  };
  for (let i = 0; i < tagTargets.length; i += REVIEW_CHUNK) {
    const chunk = tagTargets.slice(i, i + REVIEW_CHUNK);
    try {
      const review = await assistantApi.reviewAnalysisTagsBulk(chunk, "accepted");
      summary.acceptedTags += review.applied.length;
      summary.tagFailures += review.failures.length;
    } catch (error) {
      summary.tagFailures += chunk.length;
      summary.tagError = error instanceof Error ? error.message : "unknown error";
      break;
    }
    onProgress(Math.min(i + REVIEW_CHUNK, tagTargets.length), total);
  }
  try {
    for (let i = 0; i < ops.length; i += APPLY_CHUNK) {
      const result = await cleanupApi.apply(ops.slice(i, i + APPLY_CHUNK), summary.batchId, scopeLabel);
      summary.batchId = result.batch_id ?? summary.batchId;
      summary.applied += result.applied;
      summary.skipped.push(...result.skipped);
      onProgress(tagTargets.length + Math.min(i + APPLY_CHUNK, ops.length), total);
    }
  } catch (error) {
    summary.applyError = error instanceof Error ? error.message : "unknown";
  }
  return summary;
}
