import { useState } from "react";

import {
  type LibraryTagTrack,
  type AnalysisTagReviewDecision,
  type AnalysisTagReviewResult,
  type AnalysisTagSuggestion,
  type StarterTagGroup,
  assistantApi,
} from "@/core/api";
import { toast } from "@/core/toast";

import { analysisTagSuggestionKey } from "./analysisTagSelection";
import { modelStatusLabel, suggestionSource, suggestionSourceLabel } from "./tagProvenance";
import { ModelInputEvidence } from "./ModelInputEvidence";

interface AnalysisTagReviewProps {
  trackId: number;
  modelAnalysis?: LibraryTagTrack["model_analysis"];
  vocabularyGroups?: StarterTagGroup[];
  suggestions: AnalysisTagSuggestion[];
  selectedSuggestionKeys: ReadonlySet<string>;
  disabled?: boolean;
  onReviewed: (result: AnalysisTagReviewResult) => void;
  onSelectionChange: (
    suggestion: AnalysisTagSuggestion,
    selected: boolean,
  ) => void;
}

function statusLabel(status: AnalysisTagReviewDecision): string {
  if (status === "accepted") return "Accepted";
  if (status === "rejected") return "Rejected";
  return "Needs review";
}

export function AnalysisTagReview({
  trackId,
  modelAnalysis,
  vocabularyGroups = [],
  suggestions,
  selectedSuggestionKeys,
  disabled = false,
  onReviewed,
  onSelectionChange,
}: AnalysisTagReviewProps) {
  const groups = ["model", "metadata", "catalog", "other"].map((source) => ({
    source, items: suggestions.filter((suggestion) => suggestionSource(suggestion.analyzer_id) === source),
  })).filter((group) => group.items.length > 0);
  const [savingKey, setSavingKey] = useState<string | null>(null);

  async function review(
    suggestion: AnalysisTagSuggestion,
    decision: AnalysisTagReviewDecision,
  ) {
    const key = analysisTagSuggestionKey(trackId, suggestion);
    setSavingKey(key);
    try {
      const result = await assistantApi.reviewAnalysisTag(
        trackId,
        suggestion,
        decision,
      );
      onReviewed(result);
      if (decision === "accepted") {
        toast.success(
          "Tag added",
          `“${suggestion.tag}” is now in your mood library.`,
        );
      } else if (decision === "rejected") {
        toast.success(
          "Suggestion rejected",
          `“${suggestion.tag}” is marked rejected for this analysis.`,
        );
      } else {
        toast.success(
          "Decision reopened",
          "The suggestion can be reviewed again. Existing mood-library tags were not removed.",
        );
      }
    } catch (error) {
      toast.error(
        "Review decision could not be saved",
        error instanceof Error ? error.message : undefined,
      );
    } finally {
      setSavingKey(null);
    }
  }

  return (
    <div className="assistant-tag-source is-analysis">
      <div>
        <strong>Generated suggestions</strong>
        <span>
          Generated evidence stays separate. Only accepting a suggestion copies it
          into your database mood library.
        </span>
      </div>
      <div className="assistant-model-review-status">
        <strong>{modelStatusLabel(modelAnalysis)}</strong>
        {modelAnalysis?.updated_at_unix_seconds != null ? (
          <span>Saved {new Date(modelAnalysis.updated_at_unix_seconds * 1000).toLocaleString()}</span>
        ) : null}
        {modelAnalysis?.suggested_tag_count === 0 ? <strong>No supported tags returned</strong> : null}
        {modelAnalysis?.suggested_tag_count != null && modelAnalysis.suggested_tag_count > 0 ? <span>{modelAnalysis.suggested_tag_count} tags returned by this analysis; review filters may hide some.</span> : null}
        {modelAnalysis?.evidence?.length ? <div><strong>Model explanation</strong><ul>{modelAnalysis.evidence.map((value, index) => <li key={index}>{value}</li>)}</ul></div> : modelAnalysis?.suggested_tag_count === 0 ? <span>The reason was not recorded in this older result.</span> : null}
        {modelAnalysis?.context_status ? <span>Context used: {modelAnalysis.context_status === "full" ? "complete local analysis" : modelAnalysis.context_status === "partial" ? "partial local analysis" : "metadata only"}. Coverage does not measure mood accuracy.</span> : null}
        {modelAnalysis?.confidence ? <span>Model-reported confidence: {modelAnalysis.confidence}</span> : null}
        {modelAnalysis?.input_snapshot ? <ModelInputEvidence input={modelAnalysis.input_snapshot} /> : modelAnalysis?.status !== "missing" && modelAnalysis?.status ? <span>The exact input was not retained for this older result.</span> : null}
        {modelAnalysis?.status === "stale" ? <span>The saved AI result no longer matches current evidence or model settings. Its old suggestions cannot be accepted.</span> : null}
        {modelAnalysis?.status === "missing" ? <span>Local measurements and keyword guesses do not mean this track has an AI result.</span> : null}
        {modelAnalysis?.job_id ? <details><summary>AI run details</summary><code>{modelAnalysis.job_id}</code></details> : null}
      </div>
      {disabled ? (
        <p className="assistant-review-note">
          Save or discard your current mood-tag edits before reviewing suggestions.
        </p>
      ) : null}
      {suggestions.length === 0 ? (
        <p className="muted small">No generated tags available.</p>
      ) : (
        <div className="assistant-analysis-review-list">
          {groups.map((group) => (
            <section key={group.source} className="assistant-review-source-group" aria-label={suggestionSourceLabel(group.items[0]!.analyzer_id)}>
              <h3>{suggestionSourceLabel(group.items[0]!.analyzer_id)}</h3>
              {group.source === "metadata" ? <p className="muted small">Guesses from words in the title, album and genre. These are not embedded mood tags or AI detection; misleading song names can produce wrong guesses. Reject any that do not fit.</p> : null}
              {group.source === "model" ? <p className="muted small">Mood tags describe an impression. Scene and setting tags propose session uses; they do not claim the song depicts a literal event. Accept only tags you find useful for this music.</p> : null}
              {group.items.map((suggestion) => {
                const key = analysisTagSuggestionKey(trackId, suggestion);
                const saving = savingKey === key;
                return (
                  <article
                    className={`assistant-analysis-review is-${suggestion.status}`}
                    key={key}
                  >
                    <div className="assistant-analysis-review-heading">
                      <div>
                        <strong>{suggestion.tag}</strong>
                        {group.source === "model" ? <span>{(() => {
                          const kind = vocabularyGroups.find((item) => item.tags.includes(suggestion.tag))?.key;
                          return kind === "mood" ? "Musical impression" : kind === "scene" || kind === "setting" ? "Suggested session use" : kind === "period" ? "Period character" : "Custom vocabulary suggestion";
                        })()}</span> : null}
                        <span>
                          {suggestionSourceLabel(suggestion.analyzer_id)} · {suggestion.confidence} confidence
                        </span>
                      </div>
                      <span className="assistant-review-status">
                        {statusLabel(suggestion.status)}
                      </span>
                    </div>
                    {suggestion.evidence.length > 0 ? (
                      <details>
                        <summary>Why this was suggested</summary>
                        <ul>
                          {suggestion.evidence.map((evidence) => (
                            <li key={evidence}>{group.source === "metadata" ? evidence.replace(/^Mood metadata:/, "Keyword match:") : evidence}</li>
                          ))}
                        </ul>
                      </details>
                    ) : null}
                    {suggestion.status === "pending" ? (
                      <label className="assistant-review-select">
                        <input
                          type="checkbox"
                          checked={selectedSuggestionKeys.has(key)}
                          disabled={disabled || savingKey !== null}
                          aria-label={`Select ${suggestion.tag} suggestion for bulk review`}
                          onChange={(event) =>
                            onSelectionChange(suggestion, event.target.checked)
                          }
                        />
                        <span>Select for a bulk decision</span>
                      </label>
                    ) : null}
                    <div className="assistant-analysis-review-actions">
                      {suggestion.status === "pending" ? (
                        <>
                          <button
                            type="button"
                            disabled={disabled || savingKey !== null}
                            aria-label={`Reject ${suggestion.tag} suggestion`}
                            onClick={() => void review(suggestion, "rejected")}
                          >
                            Reject
                          </button>
                          <button
                            type="button"
                            className="btn-primary"
                            disabled={disabled || savingKey !== null}
                            aria-label={`Accept ${suggestion.tag} into mood library`}
                            onClick={() => void review(suggestion, "accepted")}
                          >
                            {saving ? "Saving…" : "Add to my tags"}
                          </button>
                        </>
                      ) : (
                        <button
                          type="button"
                          disabled={disabled || savingKey !== null}
                          aria-label={`Review ${suggestion.tag} again`}
                          onClick={() => void review(suggestion, "pending")}
                        >
                          {saving ? "Saving…" : "Review again"}
                        </button>
                      )}
                    </div>
                  </article>
                );
              })}
            </section>
          ))}
        </div>
      )}
      <p className="assistant-review-note">
        Reopening a decision never removes a mood-library tag you already accepted.
      </p>
    </div>
  );
}
