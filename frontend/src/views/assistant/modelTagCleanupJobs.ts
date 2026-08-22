import type {
  BackgroundJob,
  ModelTagCleanupSuggestion,
} from "@/core/api";

import { isBackgroundJobActive } from "./backgroundJobs";

export const MODEL_TAG_CLEANUP_JOB_KIND = "assistant.model-tag-cleanup";

export interface ModelTagCleanupJobResult {
  schema_version: "assistant-model-tag-cleanup-job-result/v3";
  disclosure_version: "assistant-model-tag-cleanup-disclosure/v3";
  role_id: "tag_cleanup";
  role_fingerprint: string;
  engine_id: "model-tag-cleanup/v3";
  catalog_signature: string;
  vocabulary_fingerprint: string;
  catalog_tags: number;
  suggestions: ModelTagCleanupSuggestion[];
}

const SIGNATURE = /^[a-f0-9]{64}$/;

function isCount(value: unknown): value is number {
  return typeof value === "number" && Number.isInteger(value) && value >= 0;
}

function suggestionFromUnknown(
  value: unknown,
): ModelTagCleanupSuggestion | null {
  if (typeof value !== "object" || value === null) return null;
  const item = value as Record<string, unknown>;
  if (
    typeof item.id !== "string" ||
    !SIGNATURE.test(item.id) ||
    typeof item.source !== "string" ||
    item.source.length === 0 ||
    typeof item.target !== "string" ||
    item.target.length === 0 ||
    !["local-rule", "model"].includes(String(item.origin)) ||
    !["high", "medium", "low"].includes(String(item.confidence)) ||
    typeof item.reason !== "string" ||
    item.reason.length === 0 ||
    !isCount(item.source_track_count) ||
    item.source_track_count < 1 ||
    !isCount(item.target_track_count) ||
    typeof item.merged !== "boolean"
  ) {
    return null;
  }
  return item as unknown as ModelTagCleanupSuggestion;
}

export function isModelTagCleanupJobActive(
  job: BackgroundJob | null | undefined,
): boolean {
  return isBackgroundJobActive(job);
}

export function modelTagCleanupResultFromJob(
  job: BackgroundJob | null | undefined,
): ModelTagCleanupJobResult | null {
  const result = job?.result;
  if (
    result?.schema_version !== "assistant-model-tag-cleanup-job-result/v3" ||
    result.disclosure_version !==
      "assistant-model-tag-cleanup-disclosure/v3" ||
    result.role_id !== "tag_cleanup" ||
    typeof result.role_fingerprint !== "string" ||
    !SIGNATURE.test(result.role_fingerprint) ||
    result.engine_id !== "model-tag-cleanup/v3" ||
    typeof result.catalog_signature !== "string" ||
    !SIGNATURE.test(result.catalog_signature) ||
    typeof result.vocabulary_fingerprint !== "string" ||
    !SIGNATURE.test(result.vocabulary_fingerprint) ||
    !isCount(result.catalog_tags) ||
    result.catalog_tags < 1 ||
    !Array.isArray(result.suggestions)
  ) {
    return null;
  }
  const suggestions = result.suggestions.map(suggestionFromUnknown);
  if (suggestions.some((item) => item === null)) return null;
  return {
    schema_version: result.schema_version,
    disclosure_version: result.disclosure_version,
    role_id: result.role_id,
    role_fingerprint: result.role_fingerprint,
    engine_id: result.engine_id,
    catalog_signature: result.catalog_signature,
    vocabulary_fingerprint: result.vocabulary_fingerprint,
    catalog_tags: result.catalog_tags,
    suggestions: suggestions as ModelTagCleanupSuggestion[],
  };
}
