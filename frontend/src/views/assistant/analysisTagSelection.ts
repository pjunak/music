import type { AnalysisTagSuggestion } from "@/core/api";

export function analysisTagSuggestionKey(
  trackId: number,
  suggestion: Pick<
    AnalysisTagSuggestion,
    "tag" | "analyzer_id" | "source_signature"
  >,
): string {
  return JSON.stringify([
    trackId,
    suggestion.analyzer_id,
    suggestion.source_signature,
    suggestion.tag,
  ]);
}
