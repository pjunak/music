import {
  type FormEvent,
  useCallback,
  useEffect,
  useMemo,
  useRef,
  useState,
} from "react";
import { Link } from "react-router-dom";

import { Field } from "@/components/Field";
import { confirmDialog } from "@/components/confirmDialog";
import {
  type AuthoringImportPreview,
  type BackgroundJob,
  MODEL_PLAYLIST_DISCLOSURE_VERSION,
  type ModelPlaylistAvailability,
  type PlaylistEnergyCurve,
  type PlaylistSuggestion,
  type PlaylistSuggestionRequest,
  assistantApi,
  authoringImportApi,
  jobsApi,
} from "@/core/api";
import { usePlayerStore } from "@/core/playerStore";
import { toast } from "@/core/toast";
import { wsClient } from "@/core/ws";

import { PlaylistSuggestionResults } from "./PlaylistSuggestionResults";
import { ProviderBoundaryPopover } from "./AssistantInfoPopover";
import { ModelUsageSummary } from "./ModelUsageSummary";
import {
  MODEL_PLAYLIST_SUGGESTION_JOB_KIND,
  isModelSuggestionJobActive,
  modelSuggestionFromJob,
  modelSuggestionRequestFromJob,
} from "./modelSuggestionJobs";

type PlanningMethod = "local" | "model";

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

function modelUnavailableMessage(reasonCode: string | null): string {
  switch (reasonCode) {
    case "model_quality_not_passed":
      return "Run and pass the playlist quality check in model settings first.";
    case "role_not_enabled":
    case "role_not_configured":
      return "Assign and enable a playlist planning model in model settings first.";
    case "connection_not_verified":
    case "model_not_tested":
      return "Verify and test the assigned playlist model in model settings first.";
    default:
      return "The connected model is not ready. Check its setup before using it.";
  }
}

export interface PlaylistBuilderViewProps {
  embedded?: boolean;
  onCreated?: (playlistName: string) => void | Promise<void>;
}

export function PlaylistBuilderView({
  embedded = false,
  onCreated,
}: PlaylistBuilderViewProps = {}) {
  const activeModeId = usePlayerStore((state) => state.state?.active_mode_id ?? null);
  const ambientTrackId = usePlayerStore(
    (state) => state.state?.ambient?.current_track_id ?? null,
  );
  const ambientIsPlaying = usePlayerStore(
    (state) => state.state?.is_playing ?? false,
  );
  const [prompt, setPrompt] = useState("");
  const [targetMinutes, setTargetMinutes] = useState(60);
  const [energyCurve, setEnergyCurve] = useState<PlaylistEnergyCurve>("steady");
  const [minBpm, setMinBpm] = useState("");
  const [maxBpm, setMaxBpm] = useState("");
  const [includeUnknownBpm, setIncludeUnknownBpm] = useState(true);
  const [planningMethod, setPlanningMethod] = useState<PlanningMethod>("local");
  const [modelAvailability, setModelAvailability] =
    useState<ModelPlaylistAvailability | null>(null);
  const [modelStatusLoading, setModelStatusLoading] = useState(true);
  const [modelStatusError, setModelStatusError] = useState<string | null>(null);
  const [modelJob, setModelJob] = useState<BackgroundJob | null>(null);
  const [suggestion, setSuggestion] = useState<PlaylistSuggestion | null>(null);
  const [selectedTrackIds, setSelectedTrackIds] = useState<Set<number>>(new Set());
  const [playlistName, setPlaylistName] = useState("");
  const [playlistCategory, setPlaylistCategory] = useState("");
  const [suggesting, setSuggesting] = useState(false);
  const [confirmingModel, setConfirmingModel] = useState(false);
  const [suggestError, setSuggestError] = useState<string | null>(null);
  const [previewing, setPreviewing] = useState(false);
  const [committing, setCommitting] = useState(false);
  const [preview, setPreview] = useState<AuthoringImportPreview | null>(null);
  const [previewDocument, setPreviewDocument] = useState<PlaylistImportDocument | null>(
    null,
  );
  const [created, setCreated] = useState(false);
  const userActionStarted = useRef(false);

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
  const activeModelJob =
    modelJob !== null && isModelSuggestionJobActive(modelJob) ? modelJob : null;
  const modelJobActive = activeModelJob !== null;
  const suggestionUsesModel = suggestion?.engine === "model-playlist-planner/v2";
  const importSourceName = suggestionUsesModel
    ? "Assistant model playlist builder"
    : "Assistant local playlist builder";

  const invalidatePreview = useCallback(() => {
    setPreview(null);
    setPreviewDocument(null);
    setCreated(false);
  }, []);

  const applySuggestion = useCallback(
    (result: PlaylistSuggestion) => {
      setSuggestion(result);
      setSelectedTrackIds(
        new Set(
          result.candidates
            .filter((candidate) => candidate.default_selected)
            .map((candidate) => candidate.track_id),
        ),
      );
      invalidatePreview();
    },
    [invalidatePreview],
  );

  const restoreRequest = useCallback((request: PlaylistSuggestionRequest) => {
    setPrompt(request.prompt);
    setTargetMinutes(request.target_minutes);
    setEnergyCurve(request.energy_curve ?? "steady");
    setMinBpm(request.min_bpm === undefined ? "" : String(request.min_bpm));
    setMaxBpm(request.max_bpm === undefined ? "" : String(request.max_bpm));
    setIncludeUnknownBpm(request.include_unknown_bpm ?? true);
  }, []);

  const acceptModelJob = useCallback(
    (job: BackgroundJob, restoreForm: boolean) => {
      setModelJob(job);
      if (restoreForm) {
        const request = modelSuggestionRequestFromJob(job);
        if (request !== null) restoreRequest(request);
      }
      if (isModelSuggestionJobActive(job)) {
        setPlanningMethod("model");
        setSuggestion(null);
        setSelectedTrackIds(new Set());
        setSuggestError(null);
        invalidatePreview();
        return;
      }
      if (job.status === "succeeded") {
        const restored = modelSuggestionFromJob(job);
        setPlanningMethod("model");
        if (restored === null) {
          setSuggestion(null);
          setSelectedTrackIds(new Set());
          setSuggestError(
            "The saved model draft is incomplete and cannot be reviewed. Run it again.",
          );
          return;
        }
        applySuggestion(restored);
        setSuggestError(null);
        return;
      }
      if (job.status === "failed" || job.status === "cancelled") {
        setPlanningMethod("model");
        setSuggestion(null);
        setSelectedTrackIds(new Set());
        setSuggestError(
          job.status === "cancelled"
            ? "The connected-model suggestion was cancelled."
            : job.error || "The connected model could not produce a playlist draft.",
        );
        invalidatePreview();
      }
    },
    [applySuggestion, invalidatePreview, restoreRequest],
  );

  useEffect(() => {
    let disposed = false;
    assistantApi
      .getModelPlaylistAvailability()
      .then((availability) => {
        if (disposed) return;
        setModelAvailability(availability);
        setModelStatusError(null);
      })
      .catch((error: unknown) => {
        if (!disposed) setModelStatusError(errorMessage(error));
      })
      .finally(() => {
        if (!disposed) setModelStatusLoading(false);
      });
    jobsApi
      .list({ kind: MODEL_PLAYLIST_SUGGESTION_JOB_KIND, limit: 1 })
      .then((jobs) => {
        if (
          !disposed &&
          !userActionStarted.current &&
          jobs[0] !== undefined
        ) {
          acceptModelJob(jobs[0], true);
        }
      })
      .catch(() => {
        // The local planner remains usable when saved model job history is unavailable.
      });
    return () => {
      disposed = true;
    };
  }, [acceptModelJob]);

  useEffect(() => {
    if (modelJob === null || !isModelSuggestionJobActive(modelJob)) return;
    const jobId = modelJob.id;
    let disposed = false;
    let timer: number | undefined;
    async function poll() {
      try {
        const next = await jobsApi.get(jobId);
        if (disposed) return;
        acceptModelJob(next, false);
        if (isModelSuggestionJobActive(next)) {
          timer = window.setTimeout(() => void poll(), 1500);
        }
      } catch (error) {
        if (disposed) return;
        setSuggestError(
          `Saved model progress is temporarily unavailable. ${errorMessage(error)}`,
        );
        timer = window.setTimeout(() => void poll(), 5000);
      }
    }
    timer = window.setTimeout(() => void poll(), 1200);
    return () => {
      disposed = true;
      if (timer !== undefined) window.clearTimeout(timer);
    };
  }, [acceptModelJob, modelJob]);

  function suggestionRequest(): PlaylistSuggestionRequest {
    const minimumBpm = optionalNumber(minBpm);
    const maximumBpm = optionalNumber(maxBpm);
    return {
      prompt: prompt.trim(),
      target_minutes: Math.min(600, Math.max(5, targetMinutes || 60)),
      candidate_limit: 40,
      energy_curve: energyCurve,
      include_unknown_bpm: includeUnknownBpm,
      ...(minimumBpm === undefined ? {} : { min_bpm: minimumBpm }),
      ...(maximumBpm === undefined ? {} : { max_bpm: maximumBpm }),
    };
  }

  async function runLocalSuggestion(request: PlaylistSuggestionRequest) {
    userActionStarted.current = true;
    setSuggesting(true);
    setSuggestError(null);
    setModelJob(null);
    invalidatePreview();
    try {
      applySuggestion(await assistantApi.suggestPlaylist(request));
    } catch (error) {
      setSuggestion(null);
      setSelectedTrackIds(new Set());
      setSuggestError(errorMessage(error));
    } finally {
      setSuggesting(false);
    }
  }

  async function runModelSuggestion(request: PlaylistSuggestionRequest) {
    const availability = modelAvailability;
    if (availability === null || !availability.available) {
      setSuggestError(
        modelUnavailableMessage(availability?.reason_code ?? null),
      );
      return;
    }
    userActionStarted.current = true;
    setConfirmingModel(true);
    let confirmed = false;
    try {
      confirmed = await confirmDialog({
        title: "Send this candidate pool to your connected model?",
        body: `The server will first filter your library, then send the disclosed metadata for at most ${availability.disclosure.maximum_candidates} candidates to ${availability.connection_name ?? "your provider"} (${availability.model_id ?? "the assigned model"}). No audio or file paths are sent. The provider may charge for this request.`,
        confirmLabel: "Send candidates and start",
        cancelLabel: "Keep editing",
        tone: "primary",
      });
    } finally {
      setConfirmingModel(false);
    }
    if (!confirmed) return;
    setSuggestError(null);
    setSuggestion(null);
    setSelectedTrackIds(new Set());
    invalidatePreview();
    try {
      const job = await assistantApi.startModelPlaylistSuggestion(
        request,
        MODEL_PLAYLIST_DISCLOSURE_VERSION,
      );
      acceptModelJob(job, false);
      toast.success(
        "Model suggestion queued",
        "You can leave this page; progress and the draft are stored on the server.",
      );
    } catch (error) {
      setSuggestError(errorMessage(error));
    }
  }

  async function suggest(event: FormEvent) {
    event.preventDefault();
    if (prompt.trim().length < 2) {
      setSuggestError("Describe the mood in at least two characters.");
      return;
    }
    const request = suggestionRequest();
    if (planningMethod === "model") await runModelSuggestion(request);
    else await runLocalSuggestion(request);
  }

  function changePlanningMethod(method: PlanningMethod) {
    userActionStarted.current = true;
    setPlanningMethod(method);
    setSuggestion(null);
    setSelectedTrackIds(new Set());
    setSuggestError(null);
    setModelJob(null);
    invalidatePreview();
  }

  async function cancelModelSuggestion() {
    if (modelJob === null || !isModelSuggestionJobActive(modelJob)) return;
    try {
      acceptModelJob(await jobsApi.cancel(modelJob.id), false);
    } catch (error) {
      setSuggestError(`Cancellation failed. ${errorMessage(error)}`);
    }
  }

  async function runLocalFallback() {
    changePlanningMethod("local");
    await runLocalSuggestion(suggestionRequest());
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

  function auditionTrack(trackId: number) {
    if (ambientTrackId !== trackId) {
      // A direct play normally honors the configured crossfade, which keeps
      // the outgoing song alive for the fade window. Auditioning must be a
      // hard replacement: ordered WebSocket messages stop and unload both
      // ambient channels before the requested draft song starts. Send the
      // stop even when our snapshot still looks empty so rapid clicks cannot
      // race a not-yet-observed prior audition.
      wsClient.send({ type: "ambient_stop" });
      wsClient.send({ type: "ambient_play_track", track_id: trackId });
      return;
    }
    wsClient.send({ type: ambientIsPlaying ? "pause" : "resume" });
  }

  async function reviewPlaylist() {
    if (activeModeId === null || selectedCandidates.length === 0) return;
    const name = playlistName.trim();
    if (!name) return;

    const document: PlaylistImportDocument = {
      schema: "authoring-import/v1",
      name: `${suggestionUsesModel ? "Model" : "Local"} suggestion: ${prompt.trim()}`,
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
        importSourceName,
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
    let createdName: string | null = null;
    setCommitting(true);
    try {
      const result = await authoringImportApi.commitDocument(
        activeModeId,
        previewDocument,
        [{ kind: "playlist", resource_id: previewItem.resource_id }],
        importSourceName,
      );
      if (result.imported.length !== 1) {
        const detail = result.skipped[0]?.reason ?? "The playlist was not created.";
        toast.error("Create skipped", detail);
        return;
      }
      setCreated(true);
      createdName = playlistName.trim();
      toast.success("Playlist created", `${createdName} is ready in Authoring.`);
    } catch (error) {
      toast.error("Create failed", errorMessage(error));
    } finally {
      setCommitting(false);
    }
    if (createdName !== null && onCreated !== undefined) {
      try {
        await onCreated(createdName);
      } catch (error) {
        toast.warn(
          "Playlist created but could not be opened",
          errorMessage(error),
        );
      }
    }
  }

  return (
    <div className={`assistant-playlist-view${embedded ? " is-embedded" : ""}`}>
      {!embedded ? (
        <header className="assistant-page-header">
          <div>
            <p className="assistant-eyebrow">Local by default · review-first</p>
            <h1>Build a playlist from a mood</h1>
            <p>
              Describe the scene or atmosphere, choose how to rank the matches, then
              decide exactly which songs reach the normal Authoring review.
            </p>
          </div>
          <span className="assistant-algorithm">
            {suggestion?.engine ?? "local-planner/v2"}
          </span>
        </header>
      ) : null}

      <div className="assistant-workbench">
        <aside
          className={`assistant-composer surface-card authoring-card${
            planningMethod === "model" ? " is-model-planning" : ""
          }`}
        >
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
            <fieldset className="assistant-planning-method">
              <legend>Planning method</legend>
              <div className="assistant-planning-options">
                <label
                  className={`assistant-planning-option${
                    planningMethod === "local" ? " is-selected" : ""
                  }`}
                >
                  <input
                    type="radio"
                    name="planning-method"
                    value="local"
                    checked={planningMethod === "local"}
                    disabled={modelJobActive}
                    onChange={() => changePlanningMethod("local")}
                  />
                  <span>
                    <strong>Local planner</strong>
                    <small>Fast, explainable, and never leaves this server.</small>
                  </span>
                </label>
                <label
                  className={`assistant-planning-option${
                    planningMethod === "model" ? " is-selected" : ""
                  }${
                    !modelAvailability?.available && !modelJobActive
                      ? " is-disabled"
                      : ""
                  }`}
                >
                  <input
                    type="radio"
                    name="planning-method"
                    value="model"
                    checked={planningMethod === "model"}
                    disabled={
                      modelJobActive ||
                      modelStatusLoading ||
                      !modelAvailability?.available
                    }
                    onChange={() => changePlanningMethod("model")}
                  />
                  <span>
                    <strong>Connected model</strong>
                    <small>
                      {modelStatusLoading
                        ? "Checking the certified playlist model…"
                        : modelAvailability?.available
                          ? `${modelAvailability.connection_name ?? "Provider"} · ${modelAvailability.model_id ?? "assigned model"}`
                          : "Needs a ready, quality-checked model."}
                    </small>
                  </span>
                </label>
              </div>
              {!modelStatusLoading && !modelAvailability?.available ? (
                <p className="assistant-planning-help">
                  {modelStatusError !== null
                    ? `Model status is unavailable: ${modelStatusError}`
                    : modelUnavailableMessage(modelAvailability?.reason_code ?? null)}{" "}
                  <Link to="/assistant/settings/models">Open model settings</Link>
                </p>
              ) : null}
            </fieldset>
            {planningMethod === "model" && modelAvailability !== null ? (
              <ProviderBoundaryPopover
                shared={modelAvailability.disclosure.shared_with_provider}
                neverShared={modelAvailability.disclosure.never_shared}
                footer={
                  <>
                  At most {modelAvailability.disclosure.maximum_candidates} locally
                  filtered songs. This request may incur provider cost and will run
                  only after you confirm it.
                  </>
                }
              />
            ) : null}
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
            {modelJobActive ? (
              <div className="assistant-model-progress" role="status">
                <div>
                  <strong>
                    {activeModelJob?.progress_phase || "Starting model work"}
                  </strong>
                  <span>
                    {activeModelJob?.progress_message || "The server owns this job."}
                  </span>
                </div>
                {activeModelJob?.progress_total !== null &&
                activeModelJob?.progress_total !== undefined &&
                activeModelJob.progress_total > 0 ? (
                  <progress
                    aria-label="Connected model suggestion progress"
                    value={activeModelJob.progress_current}
                    max={activeModelJob.progress_total}
                  />
                ) : (
                  <progress aria-label="Connected model suggestion progress" />
                )}
                <button
                  className="btn-ghost"
                  type="button"
                  disabled={activeModelJob?.status === "cancel_requested"}
                  onClick={() => void cancelModelSuggestion()}
                >
                  {activeModelJob?.status === "cancel_requested"
                    ? "Stopping after current step…"
                    : "Cancel model suggestion"}
                </button>
                <small>You can leave this page; progress is stored on the server.</small>
              </div>
            ) : null}
            {planningMethod === "model" &&
            suggestError !== null &&
            !modelJobActive ? (
              <button
                className="btn-secondary"
                type="button"
                onClick={() => void runLocalFallback()}
              >
                Use local planner with these settings
              </button>
            ) : null}
            <button
              className="btn-primary"
              type="submit"
              disabled={suggesting || confirmingModel || modelJobActive}
            >
              {confirmingModel
                ? "Waiting for confirmation…"
                : suggesting
                ? "Finding matches…"
                : modelJobActive
                  ? "Model suggestion running…"
                  : planningMethod === "model"
                    ? "Review disclosure and start"
                    : "Find matching songs"}
            </button>
          </form>

          <ModelUsageSummary job={modelJob} />

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
                    {created
                      ? "Playlist created"
                      : committing
                        ? "Creating…"
                        : embedded
                          ? "Create and continue editing"
                          : "Create playlist"}
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
          planningMethod={planningMethod}
          selectedTrackIds={selectedTrackIds}
          activeTrackId={ambientTrackId}
          playbackRunning={ambientIsPlaying}
          onToggleTrack={toggleTrack}
          onAuditionTrack={auditionTrack}
          onSelectAll={selectAll}
          onClearSelection={clearSelection}
        />
      </div>
    </div>
  );
}
