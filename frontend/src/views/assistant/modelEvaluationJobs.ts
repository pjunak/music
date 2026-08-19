import type { BackgroundJob, BackgroundJobStatus } from "@/core/api";

export const PLAYLIST_MODEL_ROLE_ID = "playlist_planner";
export const PLAYLIST_QUALITY_JOB_KIND =
  "assistant.model-evaluation.playlist-quality-v1";
export const MUSIC_TAGGER_ROLE_ID = "music_tagger";
export const MUSIC_TAGGING_QUALITY_JOB_KIND =
  "assistant.model-evaluation.music-tagging-quality-v1";

export const MODEL_QUALITY_TARGETS = [
  {
    roleId: PLAYLIST_MODEL_ROLE_ID,
    jobKind: PLAYLIST_QUALITY_JOB_KIND,
  },
  {
    roleId: MUSIC_TAGGER_ROLE_ID,
    jobKind: MUSIC_TAGGING_QUALITY_JOB_KIND,
  },
] as const;

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
