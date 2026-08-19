import type { BackgroundJob, BackgroundJobStatus } from "@/core/api";

export const PLAYLIST_MODEL_ROLE_ID = "playlist_planner";
export const PLAYLIST_QUALITY_JOB_KIND =
  "assistant.model-evaluation.playlist-quality-v1";

const ACTIVE_STATUSES = new Set<BackgroundJobStatus>([
  "queued",
  "running",
  "cancel_requested",
]);

export function isModelEvaluationJobActive(
  job: BackgroundJob | undefined,
): boolean {
  return job !== undefined && ACTIVE_STATUSES.has(job.status);
}
