import { useState } from "react";

import {
  type AnalysisTagReviewDecision,
  type AnalysisTagReviewResult,
  type AnalysisTagSuggestion,
  assistantApi,
} from "@/core/api";
import { toast } from "@/core/toast";

import { analysisTagSuggestionKey } from "./analysisTagSelection";

interface AnalysisTagReviewProps {
  trackId: number;
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
  suggestions,
  selectedSuggestionKeys,
  disabled = false,
  onReviewed,
  onSelectionChange,
}: AnalysisTagReviewProps) {
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
          `“${suggestion.tag}” is now one of your manual tags.`,
        );
      } else if (decision === "rejected") {
        toast.success(
          "Suggestion rejected",
          `“${suggestion.tag}” is marked rejected for this analysis.`,
        );
      } else {
        toast.success(
          "Decision reopened",
          "The suggestion can be reviewed again. Existing manual tags were not removed.",
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
          into your manual tags.
        </span>
      </div>
      {disabled ? (
        <p className="assistant-review-note">
          Save or discard your current manual-tag edits before reviewing suggestions.
        </p>
      ) : null}
      {suggestions.length === 0 ? (
        <p className="muted small">No generated tags available.</p>
      ) : (
        <div className="assistant-analysis-review-list">
          {suggestions.map((suggestion) => {
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
                    <span>
                      {suggestion.analyzer_id} · {suggestion.confidence} confidence
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
                        <li key={evidence}>{evidence}</li>
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
                        aria-label={`Accept ${suggestion.tag} as manual tag`}
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
        </div>
      )}
      <p className="assistant-review-note">
        Reopening a decision never removes a manual tag you already accepted.
      </p>
    </div>
  );
}
