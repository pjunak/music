import type { BackgroundJob } from "@/core/api";

import { isBackgroundJobActive } from "./backgroundJobs";

export const MODEL_TAGGING_JOB_KIND = "assistant.model-music-tagging";

export interface ModelTaggingJobResult {
  schema_version: "assistant-model-music-tagging-job-result/v3";
  analyzer_id: "model-evidence-tagger/v3";
  library_tracks: number;
  updated_profiles: number;
  unchanged_profiles: number;
  skipped_changed_tracks: number;
}

export function isModelTaggingJobActive(
  job: BackgroundJob | null | undefined,
): boolean {
  return isBackgroundJobActive(job);
}

export function modelTaggingResultFromJob(
  job: BackgroundJob | null | undefined,
): ModelTaggingJobResult | null {
  const result = job?.result;
  const isCount = (value: unknown): value is number =>
    typeof value === "number" && Number.isInteger(value) && value >= 0;
  if (
    result?.schema_version !== "assistant-model-music-tagging-job-result/v3" ||
    result.analyzer_id !== "model-evidence-tagger/v3" ||
    !isCount(result.library_tracks) ||
    !isCount(result.updated_profiles) ||
    !isCount(result.unchanged_profiles) ||
    !isCount(result.skipped_changed_tracks)
  ) {
    return null;
  }
  return {
    schema_version: result.schema_version,
    analyzer_id: result.analyzer_id,
    library_tracks: result.library_tracks,
    updated_profiles: result.updated_profiles,
    unchanged_profiles: result.unchanged_profiles,
    skipped_changed_tracks: result.skipped_changed_tracks,
  };
}
