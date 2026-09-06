import type { LibraryTagTrack, TagSuggestionSource } from "@/core/api";

export function suggestionSource(analyzer: string): TagSuggestionSource | "other" {
  if (analyzer.startsWith("model-context-tagger/")) return "model";
  if (analyzer.startsWith("local-metadata/")) return "metadata";
  if (analyzer.startsWith("catalog-tags/")) return "catalog";
  return "other";
}

export function suggestionSourceLabel(analyzer: string): string {
  switch (suggestionSource(analyzer)) {
    case "model": return "AI suggestions";
    case "metadata": return "Metadata keyword guesses";
    case "catalog": return "Catalog suggestions";
    default: return analyzer;
  }
}

export function modelStatusLabel(analysis: LibraryTagTrack["model_analysis"]): string {
  switch (analysis?.status) {
    case "current": return "AI processed · current";
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
