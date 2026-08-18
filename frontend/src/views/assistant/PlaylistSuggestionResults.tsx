import { EmptyState } from "@/components/EmptyState";
import type {
  PlaylistSuggestion,
  PlaylistSuggestionCandidate,
} from "@/core/api";

interface Props {
  suggestion: PlaylistSuggestion | null;
  selectedTrackIds: ReadonlySet<number>;
  onToggleTrack: (trackId: number) => void;
  onSelectAll: () => void;
  onClearSelection: () => void;
}

function displayName(candidate: PlaylistSuggestionCandidate): string {
  return candidate.display_title || candidate.title || candidate.path;
}

function MoodAxis({ label, value }: { label: string; value: number }) {
  const percent = Math.round(value * 100);
  return (
    <div className="assistant-axis">
      <div className="assistant-axis-label">
        <span>{label}</span>
        <span>{percent}%</span>
      </div>
      <div
        className="assistant-axis-track"
        role="meter"
        aria-label={label}
        aria-valuemin={0}
        aria-valuemax={100}
        aria-valuenow={percent}
      >
        <span style={{ width: `${percent}%` }} />
      </div>
    </div>
  );
}

export function PlaylistSuggestionResults({
  suggestion,
  selectedTrackIds,
  onToggleTrack,
  onSelectAll,
  onClearSelection,
}: Props) {
  if (suggestion === null) {
    return (
      <div className="assistant-results">
        <div className="surface-card assistant-results-empty">
          <EmptyState title="Your library stays private and local">
            Start with a mood. This first engine makes no remote calls and does not
            claim to hear the audio; it explains which metadata and tempo signals
            influenced each match.
          </EmptyState>
        </div>
      </div>
    );
  }

  return (
    <div className="assistant-results">
      <section className="assistant-intent surface-card" aria-label="Interpreted mood">
        <div className="assistant-section-heading">
          <div>
            <p className="assistant-eyebrow">How the request was interpreted</p>
            <h2>Mood profile</h2>
          </div>
          <span>
            {suggestion.eligible_tracks} of {suggestion.library_tracks} tracks eligible
          </span>
        </div>
        <div className="assistant-intent-grid">
          <div className="assistant-mood-tags">
            {suggestion.intent.matched_moods.length > 0 ? (
              suggestion.intent.matched_moods.map((mood) => (
                <span key={mood}>{mood}</span>
              ))
            ) : (
              <span>neutral profile</span>
            )}
            {suggestion.intent.search_terms.map((term) => (
              <span className="is-context" key={term}>
                {term}
              </span>
            ))}
          </div>
          <div className="assistant-axes">
            <MoodAxis label="Energy" value={suggestion.intent.energy} />
            <MoodAxis label="Brightness" value={suggestion.intent.brightness} />
            <MoodAxis label="Tension" value={suggestion.intent.tension} />
          </div>
        </div>
      </section>

      <section className="assistant-candidates surface-card" aria-label="Suggested songs">
        <div className="assistant-section-heading">
          <div>
            <p className="assistant-eyebrow">Ranked locally</p>
            <h2>Suggested songs</h2>
          </div>
          <div className="assistant-selection-actions">
            <button type="button" className="btn-ghost" onClick={onSelectAll}>
              Select all
            </button>
            <button type="button" className="btn-ghost" onClick={onClearSelection}>
              Clear
            </button>
          </div>
        </div>
        {suggestion.candidates.length === 0 ? (
          <EmptyState title="No songs passed these filters">
            Widen the BPM range, include tracks without BPM, or try a broader mood
            description.
          </EmptyState>
        ) : (
          <div className="assistant-candidate-list">
            {suggestion.candidates.map((candidate, index) => {
              const checked = selectedTrackIds.has(candidate.track_id);
              return (
                <label
                  className={`assistant-candidate${checked ? " is-selected" : ""}`}
                  key={candidate.track_id}
                >
                  <input
                    type="checkbox"
                    checked={checked}
                    onChange={() => onToggleTrack(candidate.track_id)}
                    aria-label={`Include ${displayName(candidate)}`}
                  />
                  <span className="assistant-rank">{index + 1}</span>
                  <span className="assistant-candidate-copy">
                    <strong>{displayName(candidate)}</strong>
                    <span className="assistant-candidate-meta">
                      {candidate.artist || "Unknown artist"}
                      {candidate.album ? ` · ${candidate.album}` : ""}
                      {candidate.bpm !== null ? ` · ${candidate.bpm} BPM` : ""}
                    </span>
                    <span className="assistant-reasons">
                      {candidate.reasons.join(" · ")}
                    </span>
                    {candidate.manual_tags.length > 0 ? (
                      <span className="assistant-candidate-tag-row is-manual">
                        <span>Your tags</span>
                        {candidate.manual_tags.slice(0, 5).map((tag) => (
                          <span key={tag}>{tag}</span>
                        ))}
                        {candidate.manual_tags.length > 5 ? (
                          <span>+{candidate.manual_tags.length - 5}</span>
                        ) : null}
                      </span>
                    ) : null}
                    {candidate.analysis_tags.length > 0 ? (
                      <span className="assistant-candidate-tag-row is-analysis">
                        <span>Analysis</span>
                        {candidate.analysis_tags.map((tag) => (
                          <span key={tag}>{tag}</span>
                        ))}
                      </span>
                    ) : null}
                  </span>
                  <span className="assistant-match">
                    <strong>{Math.round(candidate.match_score * 100)}%</strong>
                    <span className={`assistant-confidence is-${candidate.confidence}`}>
                      {candidate.confidence}
                    </span>
                  </span>
                </label>
              );
            })}
          </div>
        )}
      </section>
    </div>
  );
}
