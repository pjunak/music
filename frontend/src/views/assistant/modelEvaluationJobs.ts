import type { BackgroundJob } from "@/core/api";

import { isBackgroundJobActive } from "./backgroundJobs";

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

export function isModelEvaluationJobActive(
  job: BackgroundJob | undefined,
): boolean {
  return isBackgroundJobActive(job);
}
