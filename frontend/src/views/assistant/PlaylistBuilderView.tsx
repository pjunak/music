import { type FormEvent, useMemo, useState } from "react";
import { Link } from "react-router-dom";

import { Field } from "@/components/Field";
import {
  type AuthoringImportPreview,
  type PlaylistEnergyCurve,
  type PlaylistSuggestion,
  assistantApi,
  authoringImportApi,
} from "@/core/api";
import { usePlayerStore } from "@/core/playerStore";
import { toast } from "@/core/toast";

import { PlaylistSuggestionResults } from "./PlaylistSuggestionResults";

interface PlaylistImportDocument {
  schema: "authoring-import/v1";
  name: string;
  playlists: Array<{
    name: string;
    category: string | null;
    tracks: string[];
  }>;
}

function formatDuration(seconds: number): string {
  const safeSeconds = Math.max(0, Math.round(seconds));
  const hours = Math.floor(safeSeconds / 3600);
  const minutes = Math.floor((safeSeconds % 3600) / 60);
  if (hours > 0) return `${hours}h ${minutes}m`;
  return `${minutes}m`;
}

function errorMessage(error: unknown): string {
  return error instanceof Error ? error.message : "The request failed unexpectedly.";
}

function optionalNumber(value: string): number | undefined {
  if (value.trim() === "") return undefined;
  const parsed = Number(value);
  return Number.isFinite(parsed) ? parsed : undefined;
}

export function PlaylistBuilderView() {
  const activeModeId = usePlayerStore((state) => state.state?.active_mode_id ?? null);
  const [prompt, setPrompt] = useState("");
  const [targetMinutes, setTargetMinutes] = useState(60);
  const [energyCurve, setEnergyCurve] = useState<PlaylistEnergyCurve>("steady");
  const [minBpm, setMinBpm] = useState("");
  const [maxBpm, setMaxBpm] = useState("");
  const [includeUnknownBpm, setIncludeUnknownBpm] = useState(true);
  const [suggestion, setSuggestion] = useState<PlaylistSuggestion | null>(null);
  const [selectedTrackIds, setSelectedTrackIds] = useState<Set<number>>(new Set());
  const [playlistName, setPlaylistName] = useState("");
  const [playlistCategory, setPlaylistCategory] = useState("");
  const [suggesting, setSuggesting] = useState(false);
  const [suggestError, setSuggestError] = useState<string | null>(null);
  const [previewing, setPreviewing] = useState(false);
  const [committing, setCommitting] = useState(false);
  const [preview, setPreview] = useState<AuthoringImportPreview | null>(null);
  const [previewDocument, setPreviewDocument] = useState<PlaylistImportDocument | null>(
    null,
  );
  const [created, setCreated] = useState(false);

  const selectedCandidates = useMemo(
    () =>
      suggestion?.candidates.filter((candidate) =>
        selectedTrackIds.has(candidate.track_id),
      ) ?? [],
    [selectedTrackIds, suggestion],
  );
  const selectedDuration = selectedCandidates.reduce(
    (total, candidate) => total + candidate.length_s,
    0,
  );
  const previewItem = preview?.items.find((item) => item.kind === "playlist") ?? null;

  function invalidatePreview() {
    setPreview(null);
    setPreviewDocument(null);
    setCreated(false);
  }

  async function suggest(event: FormEvent) {
    event.preventDefault();
    if (prompt.trim().length < 2) {
      setSuggestError("Describe the mood in at least two characters.");
      return;
    }
    setSuggesting(true);
    setSuggestError(null);
    invalidatePreview();
    try {
      const minimumBpm = optionalNumber(minBpm);
      const maximumBpm = optionalNumber(maxBpm);
      const result = await assistantApi.suggestPlaylist({
        prompt: prompt.trim(),
        target_minutes: Math.min(600, Math.max(5, targetMinutes || 60)),
        candidate_limit: 40,
        energy_curve: energyCurve,
        include_unknown_bpm: includeUnknownBpm,
        ...(minimumBpm === undefined ? {} : { min_bpm: minimumBpm }),
        ...(maximumBpm === undefined ? {} : { max_bpm: maximumBpm }),
      });
      setSuggestion(result);
      setSelectedTrackIds(
        new Set(
          result.candidates
            .filter((candidate) => candidate.default_selected)
            .map((candidate) => candidate.track_id),
        ),
      );
    } catch (error) {
      setSuggestion(null);
      setSelectedTrackIds(new Set());
      setSuggestError(errorMessage(error));
    } finally {
      setSuggesting(false);
    }
  }

  function toggleTrack(trackId: number) {
    setSelectedTrackIds((current) => {
      const next = new Set(current);
      if (next.has(trackId)) next.delete(trackId);
      else next.add(trackId);
      return next;
    });
    invalidatePreview();
  }

  function selectAll() {
    setSelectedTrackIds(
      new Set(suggestion?.candidates.map((candidate) => candidate.track_id) ?? []),
    );
    invalidatePreview();
  }

  function clearSelection() {
    setSelectedTrackIds(new Set());
    invalidatePreview();
  }

  async function reviewPlaylist() {
    if (activeModeId === null || selectedCandidates.length === 0) return;
    const name = playlistName.trim();
    if (!name) return;

    const document: PlaylistImportDocument = {
      schema: "authoring-import/v1",
      name: `Local suggestion: ${prompt.trim()}`,
      playlists: [
        {
          name,
          category: playlistCategory.trim() || null,
          tracks: selectedCandidates.map((candidate) => candidate.path),
        },
      ],
    };
    setPreviewing(true);
    setPreview(null);
    setCreated(false);
    try {
      const result = await authoringImportApi.previewDocument(
        activeModeId,
        document,
        "Assistant local playlist builder",
      );
      setPreview(result);
      setPreviewDocument(document);
    } catch (error) {
      toast.error("Review failed", errorMessage(error));
    } finally {
      setPreviewing(false);
    }
  }

  async function createPlaylist() {
    if (
      activeModeId === null ||
      previewDocument === null ||
      previewItem?.status !== "ready"
    ) {
      return;
    }
    setCommitting(true);
    try {
      const result = await authoringImportApi.commitDocument(
        activeModeId,
        previewDocument,
        [{ kind: "playlist", resource_id: previewItem.resource_id }],
        "Assistant local playlist builder",
      );
      if (result.imported.length !== 1) {
        const detail = result.skipped[0]?.reason ?? "The playlist was not created.";
        toast.error("Create skipped", detail);
        return;
      }
      setCreated(true);
      toast.success("Playlist created", `${playlistName.trim()} is ready in Authoring.`);
    } catch (error) {
      toast.error("Create failed", errorMessage(error));
    } finally {
      setCommitting(false);
    }
  }

  return (
    <div className="assistant-playlist-view">
      <header className="assistant-page-header">
        <div>
          <p className="assistant-eyebrow">Local · explainable · review-first</p>
          <h1>Build a playlist from a mood</h1>
          <p>
            Describe the scene or atmosphere. The local planner combines your tags,
            current metadata profiles, and available audio measurements, then you
            decide exactly what gets created.
          </p>
        </div>
        <span className="assistant-algorithm">
          {suggestion?.engine ?? "local-planner/v2"}
        </span>
      </header>

      <div className="assistant-workbench">
        <aside className="assistant-composer surface-card authoring-card">
          <form className="assistant-form" onSubmit={suggest}>
            <Field
              label="Mood or scene"
              hint="Try: tense rainy investigation, calm tavern, or triumphant battle."
              error={suggestError}
            >
              <textarea
                aria-label="Mood or scene"
                value={prompt}
                onChange={(event) => setPrompt(event.target.value)}
                placeholder="Tense rainy investigation with no vocals"
                rows={4}
                maxLength={500}
              />
            </Field>
            <Field label="Target length">
              <div className="assistant-number-field">
                <input
                  type="number"
                  value={targetMinutes}
                  min={5}
                  max={600}
                  onChange={(event) => setTargetMinutes(Number(event.target.value))}
                />
                <span>minutes</span>
              </div>
            </Field>
            <Field
              label="Playlist flow"
              hint="Controls the order of the initially selected songs; you can still change every selection."
            >
              <select
                aria-label="Playlist flow"
                value={energyCurve}
                onChange={(event) =>
                  setEnergyCurve(event.target.value as PlaylistEnergyCurve)
                }
              >
                <option value="steady">Steady — keep a consistent atmosphere</option>
                <option value="rising">Rising — build intensity</option>
                <option value="falling">Falling — wind down gradually</option>
                <option value="arc">Arc — build to a peak, then resolve</option>
              </select>
            </Field>
            <details className="assistant-filters">
              <summary>Optional tempo filters</summary>
              <div className="assistant-filter-grid">
                <Field label="Minimum BPM">
                  <input
                    type="number"
                    value={minBpm}
                    min={1}
                    max={999}
                    onChange={(event) => setMinBpm(event.target.value)}
                  />
                </Field>
                <Field label="Maximum BPM">
                  <input
                    type="number"
                    value={maxBpm}
                    min={1}
                    max={999}
                    onChange={(event) => setMaxBpm(event.target.value)}
                  />
                </Field>
              </div>
              <label className="assistant-check-row">
                <input
                  type="checkbox"
                  checked={includeUnknownBpm}
                  onChange={(event) => setIncludeUnknownBpm(event.target.checked)}
                />
                Include songs without BPM data
              </label>
            </details>
            <button className="btn-primary" type="submit" disabled={suggesting}>
              {suggesting ? "Finding matches…" : "Find matching songs"}
            </button>
          </form>

          {suggestion !== null ? (
            <section className="assistant-create-panel" aria-label="Create playlist">
              <div className="assistant-create-summary">
                <strong>{selectedCandidates.length} selected</strong>
                <span>{formatDuration(selectedDuration)}</span>
              </div>
              <Field label="Playlist name">
                <input
                  aria-label="Playlist name"
                  value={playlistName}
                  maxLength={256}
                  onChange={(event) => {
                    setPlaylistName(event.target.value);
                    invalidatePreview();
                  }}
                  placeholder="Rainy investigation"
                />
              </Field>
              <Field label="Category" hint="Optional grouping used in Authoring.">
                <input
                  aria-label="Category"
                  value={playlistCategory}
                  maxLength={64}
                  onChange={(event) => {
                    setPlaylistCategory(event.target.value);
                    invalidatePreview();
                  }}
                  placeholder="exploration"
                />
              </Field>
              {activeModeId === null ? (
                <p className="assistant-warning">Pick an active mode before creating.</p>
              ) : null}
              <button
                className="btn-secondary"
                type="button"
                onClick={() => void reviewPlaylist()}
                disabled={
                  previewing ||
                  selectedCandidates.length === 0 ||
                  playlistName.trim() === "" ||
                  activeModeId === null
                }
              >
                {previewing ? "Checking playlist…" : "Review playlist"}
              </button>
              {previewItem !== null ? (
                <div className={`assistant-import-review is-${previewItem.status}`}>
                  <div>
                    <strong>
                      {previewItem.status === "ready" ? "Ready to create" : "Needs attention"}
                    </strong>
                    <span>{previewItem.summary}</span>
                  </div>
                  {previewItem.reason ? <p>{previewItem.reason}</p> : null}
                  {previewItem.issues.map((issue) => (
                    <p key={`${issue.code}-${issue.message}`}>{issue.message}</p>
                  ))}
                  <button
                    className="btn-primary"
                    type="button"
                    onClick={() => void createPlaylist()}
                    disabled={previewItem.status !== "ready" || committing || created}
                  >
                    {created ? "Playlist created" : committing ? "Creating…" : "Create playlist"}
                  </button>
                  {created ? (
                    <Link className="btn-link" to="/authoring/playlists">
                      Open in Authoring
                    </Link>
                  ) : null}
                </div>
              ) : null}
            </section>
          ) : null}
        </aside>

        <PlaylistSuggestionResults
          suggestion={suggestion}
          selectedTrackIds={selectedTrackIds}
          onToggleTrack={toggleTrack}
          onSelectAll={selectAll}
          onClearSelection={clearSelection}
        />
      </div>
    </div>
  );
}
