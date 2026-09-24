import type { LibraryTagTrack, TagSuggestionSource } from "@/core/api";

export function suggestionSource(analyzer: string): TagSuggestionSource | "other" {
  if (analyzer === "model-context-tagger/v8") return "model";
  if (analyzer === "catalog-tags/v1") return "catalog";
  return "other";
}

export function suggestionSourceLabel(analyzer: string): string {
  switch (suggestionSource(analyzer)) {
    case "model": return "AI suggestions";
    case "catalog": return "Catalog suggestions";
    default: return analyzer;
  }
}

export function modelStatusLabel(analysis: LibraryTagTrack["model_analysis"]): string {
  switch (analysis?.status) {
    case "current": return analysis.suggested_tag_count === 0 ? "AI processed · no tags" : "AI processed · current";
    case "stale": return "AI processed · outdated";
    case "missing": return "No saved AI result";
    default: return "AI status unavailable";
  }
}

export function modelRunReviewUrl(jobId?: string): string {
  const query = new URLSearchParams({ model_status: "processed", suggestion_source: "model" });
  if (jobId) query.set("model_job_id", jobId);
  return `/assistant/moods/tags?${query.toString()}`;
}
