import type {
  BackgroundJob,
  ModelTaggingContextPolicy,
  ModelTaggingScope,
} from "@/core/api";

import { isBackgroundJobActive } from "./backgroundJobs";

export const MODEL_TAGGING_JOB_KIND = "assistant.model-music-tagging";

export interface ModelTaggingJobResult {
  schema_version: "assistant-model-music-tagging-job-result/v6" | "assistant-model-music-tagging-job-result/v7";
  analyzer_id: "model-context-tagger/v6" | "model-context-tagger/v7";
  vocabulary_fingerprint: string;
  library_tracks: number;
  scope_tracks: number;
  updated_profiles: number;
  unchanged_profiles: number;
  skipped_changed_tracks: number;
  context_policy: ModelTaggingContextPolicy;
  skipped_context_tracks: number;
  deferred_tracks?: number;
  processed_tracks?: number;
  tracks_with_suggestions?: number;
  tracks_without_suggestions?: number;
  suggested_tags?: number;
  stopped_empty_batch?: boolean;
  remaining_tracks?: number;
}

export function isModelTaggingJobActive(
  job: BackgroundJob | null | undefined,
): boolean {
  return isBackgroundJobActive(job);
}

export function modelTaggingScopeFromJob(
  job: BackgroundJob | null | undefined,
): ModelTaggingScope | null {
  const value = job?.parameters.scope;
  if (typeof value !== "object" || value === null) return null;
  const candidate = value as Record<string, unknown>;
  if (candidate.type === "all") return { type: "all" };
  if (
    candidate.type === "folder" &&
    typeof candidate.path === "string" &&
    typeof candidate.recursive === "boolean"
  ) {
    return {
      type: "folder",
      path: candidate.path,
      recursive: candidate.recursive,
    };
  }
  if (
    candidate.type === "tracks" &&
    Array.isArray(candidate.track_ids) &&
    candidate.track_ids.length > 0 &&
    candidate.track_ids.every(
      (trackId) => Number.isInteger(trackId) && Number(trackId) > 0,
    )
  ) {
    return {
      type: "tracks",
      track_ids: candidate.track_ids.map(Number),
    };
  }
  return null;
}

export function modelTaggingResultFromJob(
  job: BackgroundJob | null | undefined,
): ModelTaggingJobResult | null {
  const result = job?.result;
  const isCount = (value: unknown): value is number =>
    typeof value === "number" && Number.isInteger(value) && value >= 0;
  if (
    (result?.schema_version !== "assistant-model-music-tagging-job-result/v6" && result?.schema_version !== "assistant-model-music-tagging-job-result/v7") ||
    (result.analyzer_id !== "model-context-tagger/v6" && result.analyzer_id !== "model-context-tagger/v7") ||
    typeof result.vocabulary_fingerprint !== "string" ||
    !/^[a-f0-9]{64}$/.test(result.vocabulary_fingerprint) ||
    !isCount(result.library_tracks) ||
    !isCount(result.scope_tracks) ||
    !isCount(result.updated_profiles) ||
    !isCount(result.unchanged_profiles) ||
    !isCount(result.skipped_changed_tracks) ||
    (result.context_policy !== "include" && result.context_policy !== "skip") ||
    !isCount(result.skipped_context_tracks)
  ) {
    return null;
  }
  if (result.schema_version === "assistant-model-music-tagging-job-result/v7" && (
    !isCount(result.processed_tracks) || !isCount(result.tracks_with_suggestions) ||
    !isCount(result.tracks_without_suggestions) || !isCount(result.suggested_tags) ||
    !isCount(result.remaining_tracks) || !isCount(result.deferred_tracks) ||
    typeof result.stopped_empty_batch !== "boolean" ||
    result.processed_tracks !== result.tracks_with_suggestions + result.tracks_without_suggestions
  )) return null;
  return {
    schema_version: result.schema_version,
    analyzer_id: result.analyzer_id,
    vocabulary_fingerprint: result.vocabulary_fingerprint,
    library_tracks: result.library_tracks,
    scope_tracks: result.scope_tracks,
    updated_profiles: result.updated_profiles,
    unchanged_profiles: result.unchanged_profiles,
    skipped_changed_tracks: result.skipped_changed_tracks,
    context_policy: result.context_policy,
    skipped_context_tracks: result.skipped_context_tracks,
    ...(result.schema_version === "assistant-model-music-tagging-job-result/v7" ? {
      processed_tracks: result.processed_tracks as number,
      tracks_with_suggestions: result.tracks_with_suggestions as number,
      tracks_without_suggestions: result.tracks_without_suggestions as number,
      suggested_tags: result.suggested_tags as number,
      remaining_tracks: result.remaining_tracks as number,
      stopped_empty_batch: result.stopped_empty_batch as boolean,
      deferred_tracks: result.deferred_tracks as number,
    } : {}),
  };
}
