import type { LibraryTagPage, LibraryTagTrack } from "./api";

function record(value: unknown): value is Record<string, unknown> {
  return value !== null && typeof value === "object" && !Array.isArray(value);
}
function strings(value: unknown, minimum: number, maximum: number, length: number): boolean {
  return Array.isArray(value) && value.length >= minimum && value.length <= maximum
    && value.every((item) => typeof item === "string" && item.trim().length > 0 && [...item].length <= length);
}
function currentSuggestion(value: unknown): boolean {
  if (!record(value)) return false;
  const fields = ["tag", "analyzer_id", "source_signature", "support", "evidence", "evidence_ids", "contradiction_ids", "status"];
  return Object.keys(value).length === fields.length && Object.keys(value).every((key) => fields.includes(key))
    && typeof value.tag === "string" && value.tag.length > 0 && [...value.tag].length <= 64
    && (value.analyzer_id === "model-context-tagger/v8" || value.analyzer_id === "catalog-tags/v1")
    && typeof value.source_signature === "string" && /^[a-f0-9]{64}$/.test(value.source_signature)
    && (value.support === "supported" || value.support === "tentative")
    && strings(value.evidence, 1, 4, 512) && strings(value.evidence_ids, 1, 4, 128)
    && strings(value.contradiction_ids, 0, 4, 128)
    && (value.status === "pending" || value.status === "accepted" || value.status === "rejected");
}
function currentTrack(value: unknown): boolean {
  return record(value)
    && !["analysis_analyzer", "analysis_confidence", "audio_signal"].some((key) => key in value)
    && Array.isArray(value.analysis_suggestions) && value.analysis_suggestions.every(currentSuggestion)
    && (!record(value.model_analysis) || !("confidence" in value.model_analysis));
}
function mismatch(): never {
  throw new Error("Song analysis data does not match this app version. Refresh after updating the server.");
}
/** Reject the superseded analysis shape instead of interpreting missing support as uncertainty. */
export function requireCurrentTagTrack(track: LibraryTagTrack): LibraryTagTrack {
  return currentTrack(track) ? track : mismatch();
}
export function requireCurrentTagPage(page: LibraryTagPage): LibraryTagPage {
  return record(page) && Array.isArray(page.items) && page.items.every(currentTrack) ? page : mismatch();
}
