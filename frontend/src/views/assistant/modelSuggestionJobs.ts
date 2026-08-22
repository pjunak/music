import type {
  BackgroundJob,
  PlaylistEnergyCurve,
  PlaylistSuggestion,
  PlaylistSuggestionRequest,
} from "@/core/api";

import { isBackgroundJobActive } from "./backgroundJobs";

export const MODEL_PLAYLIST_SUGGESTION_JOB_KIND =
  "assistant.model-playlist-suggestion";

const ENERGY_CURVES = new Set<PlaylistEnergyCurve>([
  "steady",
  "rising",
  "falling",
  "arc",
]);

function objectValue(value: unknown): Record<string, unknown> | null {
  return typeof value === "object" && value !== null && !Array.isArray(value)
    ? (value as Record<string, unknown>)
    : null;
}

export function isModelSuggestionJobActive(
  job: BackgroundJob | null,
): boolean {
  return isBackgroundJobActive(job);
}

export function modelSuggestionFromJob(
  job: BackgroundJob,
): PlaylistSuggestion | null {
  if (job.status !== "succeeded") return null;
  const result = objectValue(job.result);
  if (
    result?.schema_version !== "assistant-playlist-suggestion-job-result/v1"
  ) {
    return null;
  }
  const suggestion = objectValue(result.suggestion);
  if (
    suggestion === null ||
    suggestion.engine !== "model-playlist-planner/v2" ||
    !Array.isArray(suggestion.candidates) ||
    objectValue(suggestion.intent) === null ||
    objectValue(suggestion.plan) === null
  ) {
    return null;
  }
  return suggestion as unknown as PlaylistSuggestion;
}

export function modelSuggestionRequestFromJob(
  job: BackgroundJob,
): PlaylistSuggestionRequest | null {
  const request = objectValue(job.parameters.request);
  if (
    request === null ||
    typeof request.prompt !== "string" ||
    typeof request.target_minutes !== "number"
  ) {
    return null;
  }
  if (
    request.energy_curve !== undefined &&
    (!ENERGY_CURVES.has(request.energy_curve as PlaylistEnergyCurve) ||
      typeof request.energy_curve !== "string")
  ) {
    return null;
  }
  return request as unknown as PlaylistSuggestionRequest;
}
