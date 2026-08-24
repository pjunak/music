import { useCallback, useEffect, useMemo, useState } from "react";
import { Link } from "react-router-dom";

import { confirmDialog } from "@/components/confirmDialog";
import { EmptyState } from "@/components/EmptyState";
import { PauseIcon, PlayIcon } from "@/components/icons";
import { Modal } from "@/components/Modal";
import {
  type AnalysisTagReviewTarget,
  type BackgroundJob,
  type LibraryTagPage,
  MODEL_TAGGING_DISCLOSURE_VERSION,
  type ModelTaggingAvailability,
  type ModelTaggingContextPolicy,
  type ModelTaggingScope,
  assistantApi,
  jobsApi,
} from "@/core/api";
import { usePlayerStore } from "@/core/playerStore";
import { toast } from "@/core/toast";
import { wsClient } from "@/core/ws";
import {
  MODEL_TAGGING_JOB_KIND,
  isModelTaggingJobActive,
  modelTaggingResultFromJob,
  modelTaggingScopeFromJob,
} from "@/views/assistant/modelTaggingJobs";

type ScopeType = ModelTaggingScope["type"];
type Step = "configure" | "running" | "review" | "done";

const PAGE_SIZE = 50;

function errorMessage(error: unknown): string {
  return error instanceof Error ? error.message : "The request failed unexpectedly.";
}

function suggestionKey(target: AnalysisTagReviewTarget): string {
  return JSON.stringify([
    target.track_id,
    target.analyzer_id,
    target.source_signature,
    target.tag,
  ]);
}

function describeScope(scope: ModelTaggingScope): string {
  if (scope.type === "tracks") {
    return `${scope.track_ids.length} selected track${scope.track_ids.length === 1 ? "" : "s"}`;
  }
  if (scope.type === "folder") {
    return `folder “${scope.path || "(root)"}”${scope.recursive ? " and subfolders" : " only"}`;
  }
  return "the entire library";
}

function unavailableMessage(reasonCode: string | null): string {
  switch (reasonCode) {
    case "model_quality_not_passed":
      return "Run and pass the music-tagging quality check in AI Setup first.";
    case "role_not_enabled":
    case "role_not_configured":
      return "Assign and enable a music-tagging model in AI Setup first.";
    case "connection_not_verified":
    case "model_not_tested":
      return "Verify and test the assigned music-tagging model in AI Setup first.";
    default:
      return "The connected music-tagging model is not ready yet.";
  }
}

export function MoodTaggingDialog({
  path,
  checkedIds,
  onClose,
  onChanged,
}: {
  path: string;
  checkedIds: number[];
  onClose: () => void;
  onChanged: () => void;
}) {
  const [step, setStep] = useState<Step>("configure");
  const [scopeType, setScopeType] = useState<ScopeType>(
    checkedIds.length > 0 ? "tracks" : path ? "folder" : "all",
  );
  const [recursive, setRecursive] = useState(true);
  const [force, setForce] = useState(false);
  const [contextPolicy, setContextPolicy] =
    useState<ModelTaggingContextPolicy>("include");
  const [plan, setPlan] = useState<ModelTaggingAvailability | null>(null);
  const [planError, setPlanError] = useState<string | null>(null);
  const [planLoading, setPlanLoading] = useState(true);
  const [job, setJob] = useState<BackgroundJob | null>(null);
  const [page, setPage] = useState<LibraryTagPage | null>(null);
  const [offset, setOffset] = useState(0);
  const [selected, setSelected] = useState<Set<string>>(new Set());
  const [busy, setBusy] = useState(false);
  const [reviewError, setReviewError] = useState<string | null>(null);
  const ambientTrackId = usePlayerStore(
    (state) => state.state?.ambient?.current_track_id ?? null,
  );
  const ambientIsPlaying = usePlayerStore(
    (state) => state.state?.is_playing ?? false,
  );

  const scope = useMemo<ModelTaggingScope>(
    () =>
      scopeType === "tracks"
        ? { type: "tracks", track_ids: checkedIds }
        : scopeType === "folder"
          ? { type: "folder", path, recursive }
          : { type: "all" },
    [checkedIds, path, recursive, scopeType],
  );
  const [reviewScope, setReviewScope] = useState<ModelTaggingScope>(scope);
  const scopeLabel = describeScope(scope);
  const reviewScopeLabel = describeScope(reviewScope);

  useEffect(() => {
    let disposed = false;
    void jobsApi
      .list({ kind: MODEL_TAGGING_JOB_KIND, limit: 1 })
      .then((history) => {
        const latest = history[0];
        if (disposed || !isModelTaggingJobActive(latest)) return;
        setReviewScope(modelTaggingScopeFromJob(latest) ?? { type: "all" });
        setJob(latest);
        setStep("running");
      })
      .catch(() => {
        // Job restoration is a convenience; the scoped planner remains usable.
      });
    return () => {
      disposed = true;
    };
  }, []);

  useEffect(() => {
    if (step !== "configure") return;
    let disposed = false;
    setPlanLoading(true);
    setPlanError(null);
    void assistantApi
      .planModelTagging(scope, contextPolicy)
      .then((next) => {
        if (!disposed) setPlan(next);
      })
      .catch((error: unknown) => {
        if (!disposed) {
          setPlan(null);
          setPlanError(errorMessage(error));
        }
      })
      .finally(() => {
        if (!disposed) setPlanLoading(false);
      });
    return () => {
      disposed = true;
    };
  }, [contextPolicy, scope, step]);

  const loadReview = useCallback(
    async (nextOffset: number, targetScope: ModelTaggingScope = reviewScope) => {
      setReviewError(null);
      try {
        let result = await assistantApi.queryModelLibraryTags(
          targetScope,
          "pending",
          nextOffset,
          PAGE_SIZE,
        );
        if (result.items.length === 0 && nextOffset > 0 && result.total > 0) {
          const lastOffset = Math.floor((result.total - 1) / PAGE_SIZE) * PAGE_SIZE;
          result = await assistantApi.queryModelLibraryTags(
            targetScope,
            "pending",
            lastOffset,
            PAGE_SIZE,
          );
          nextOffset = lastOffset;
        }
        setOffset(nextOffset);
        setReviewScope(targetScope);
        setPage(result);
        setSelected(
          new Set(
            result.items.flatMap((track) =>
              track.analysis_suggestions
                .filter((suggestion) => suggestion.confidence !== "low")
                .map((suggestion) =>
                  suggestionKey({
                    track_id: track.track_id,
                    tag: suggestion.tag,
                    analyzer_id: suggestion.analyzer_id,
                    source_signature: suggestion.source_signature,
                  }),
                ),
            ),
          ),
        );
        setStep("review");
      } catch (error) {
        setReviewError(errorMessage(error));
        setStep("review");
      }
    },
    [reviewScope],
  );

  useEffect(() => {
    if (step !== "running" || job === null || !isModelTaggingJobActive(job)) return;
    let disposed = false;
    const timer = window.setTimeout(() => {
      void jobsApi
        .get(job.id)
        .then((next) => {
          if (disposed) return;
          setJob(next);
          if (next.status === "succeeded") {
            void loadReview(
              0,
              modelTaggingScopeFromJob(next) ?? reviewScope,
            );
          }
        })
        .catch((error: unknown) => {
          if (!disposed) toast.error("Could not refresh tagging progress", errorMessage(error));
        });
    }, 1200);
    return () => {
      disposed = true;
      window.clearTimeout(timer);
    };
  }, [job, loadReview, reviewScope, step]);

  const visibleTargets = useMemo(
    () =>
      page?.items.flatMap((track) =>
        track.analysis_suggestions.map((suggestion) => ({
          track_id: track.track_id,
          tag: suggestion.tag,
          analyzer_id: suggestion.analyzer_id,
          source_signature: suggestion.source_signature,
        })),
      ) ?? [],
    [page],
  );
  const selectedTargets = visibleTargets.filter((target) =>
    selected.has(suggestionKey(target)),
  );

  async function startTagging() {
    if (plan === null || !plan.available || plan.scope_tracks === 0) return;
    const workTracks = force ? plan.planned_tracks : plan.tracks_needing_tags;
    const requests = force
      ? Math.ceil(plan.planned_tracks / plan.disclosure.tracks_per_request)
      : plan.estimated_provider_requests;
    if (workTracks === 0) {
      setReviewScope(scope);
      await loadReview(0, scope);
      return;
    }
    const confirmed = await confirmDialog({
      title: "Create mood-library suggestions?",
      body:
        `${workTracks} track${workTracks === 1 ? "" : "s"} in ${scopeLabel} will use about ` +
        `${requests} provider request${requests === 1 ? "" : "s"}. Titles, file and folder names from the library-relative path, descriptive metadata, and bounded time-aware local context may be sent. Audio, waveforms, full-resolution timelines, the absolute media root, file-embedded tags beyond the disclosed metadata, and your database mood tags stay local. Results remain proposals until you accept them here.`,
      confirmLabel: workTracks === 0 ? "Check current suggestions" : "Create suggestions",
      tone: "primary",
    });
    if (!confirmed) return;
    setBusy(true);
    try {
      const started = await assistantApi.startModelTagging(
        force,
        MODEL_TAGGING_DISCLOSURE_VERSION,
        scope,
        contextPolicy,
      );
      setReviewScope(scope);
      setJob(started);
      setStep("running");
      if (started.status === "succeeded") void loadReview(0, scope);
    } catch (error) {
      toast.error("Mood tagging could not start", errorMessage(error));
    } finally {
      setBusy(false);
    }
  }

  async function cancelJob() {
    if (job === null || !isModelTaggingJobActive(job)) return;
    setBusy(true);
    try {
      setJob(await jobsApi.cancel(job.id));
    } catch (error) {
      toast.error("Cancellation failed", errorMessage(error));
    } finally {
      setBusy(false);
    }
  }

  function audition(trackId: number) {
    if (ambientTrackId !== trackId) {
      wsClient.send({ type: "ambient_stop" });
      wsClient.send({ type: "ambient_play_track", track_id: trackId });
      return;
    }
    wsClient.send({ type: ambientIsPlaying ? "pause" : "resume" });
  }

  function selectVisible(mode: "all" | "confident" | "none") {
    if (mode === "none") {
      setSelected(new Set());
      return;
    }
    const allowed = new Set(
      page?.items.flatMap((track) =>
        track.analysis_suggestions
          .filter((suggestion) => mode === "all" || suggestion.confidence !== "low")
          .map((suggestion) =>
            suggestionKey({
              track_id: track.track_id,
              tag: suggestion.tag,
              analyzer_id: suggestion.analyzer_id,
              source_signature: suggestion.source_signature,
            }),
          ),
      ) ?? [],
    );
    setSelected(allowed);
  }

  async function reviewSelected(decision: "accepted" | "rejected") {
    if (selectedTargets.length === 0) return;
    setBusy(true);
    try {
      const result = await assistantApi.reviewAnalysisTagsBulk(
        selectedTargets,
        decision,
      );
      if (result.failures.length > 0) {
        toast.warn(
          "Some suggestions changed",
          `${result.failures.length} item${result.failures.length === 1 ? " was" : "s were"} skipped. The review list has been refreshed.`,
        );
      } else {
        toast.success(
          decision === "accepted" ? "Mood tags added" : "Suggestions rejected",
          `${result.applied.length} suggestion${result.applied.length === 1 ? "" : "s"} reviewed.`,
        );
      }
      onChanged();
      await loadReview(offset);
    } catch (error) {
      toast.error("Review decisions could not be saved", errorMessage(error));
    } finally {
      setBusy(false);
    }
  }

  const configureBody = (
    <div className="mood-tagging-options">
      <section>
        <h3 className="section-label">Where to create suggestions</h3>
        <div className="cleanup-scope">
          <label className="cleanup-choice">
            <input
              type="radio"
              name="mood-tagging-scope"
              checked={scopeType === "all"}
              onChange={() => setScopeType("all")}
            />
            <span>Entire library</span>
          </label>
          <label className="cleanup-choice">
            <input
              type="radio"
              name="mood-tagging-scope"
              checked={scopeType === "folder"}
              onChange={() => setScopeType("folder")}
            />
            <span>
              Current folder <strong>{path || "(root)"}</strong>
            </span>
            {scopeType === "folder" ? (
              <label className="cleanup-subchoice">
                <input
                  type="checkbox"
                  checked={recursive}
                  onChange={(event) => setRecursive(event.target.checked)}
                />
                <span className="muted">include subfolders</span>
              </label>
            ) : null}
          </label>
          <label className={`cleanup-choice${checkedIds.length === 0 ? " disabled" : ""}`}>
            <input
              type="radio"
              name="mood-tagging-scope"
              disabled={checkedIds.length === 0}
              checked={scopeType === "tracks"}
              onChange={() => setScopeType("tracks")}
            />
            <span>
              Selected tracks <span className="muted">({checkedIds.length || "none ticked"})</span>
            </span>
          </label>
        </div>
      </section>

      <section className="mood-tagging-plan" aria-live="polite">
        <div>
          <h3 className="section-label">Planned run</h3>
          {planLoading ? (
            <p className="muted">Checking this scope…</p>
          ) : planError !== null ? (
            <p className="error">{planError}</p>
          ) : plan !== null ? (
            <div className="mood-tagging-stats">
              <span><strong>{plan.scope_tracks}</strong> tracks in scope</span>
              <span><strong>{plan.planned_tracks}</strong> eligible for this run</span>
              <span><strong>{plan.estimated_provider_requests}</strong> provider requests</span>
              <span><strong>{plan.tracks_with_full_context}</strong> have full context</span>
            </div>
          ) : null}
        </div>
        {plan !== null && !plan.available ? (
          <div className="mood-tagging-unavailable">
            <strong>Mood tagging is not ready</strong>
            <span>{unavailableMessage(plan.reason_code)}</span>
            <Link to="/assistant/ai" onClick={onClose}>Open AI Setup</Link>
          </div>
        ) : null}
      </section>

      {plan !== null &&
      plan.tracks_with_partial_context + plan.tracks_missing_context > 0 ? (
        <section className="mood-tagging-context-warning" role="alert">
          <div>
            <strong>Some tracks do not have full analysis context</strong>
            <p>
              {plan.tracks_with_partial_context} partial and {plan.tracks_missing_context} missing or stale.
              You can still run the model, or limit this run to fully analyzed tracks.
            </p>
          </div>
          <div className="cleanup-scope">
            <label className="cleanup-choice">
              <input
                type="radio"
                name="mood-tagging-context-policy"
                checked={contextPolicy === "include"}
                onChange={() => setContextPolicy("include")}
              />
              <span>
                Run anyway
                <span className="cleanup-hint muted">
                  The model receives metadata and path context for tracks without full analysis.
                </span>
              </span>
            </label>
            <label className="cleanup-choice">
              <input
                type="radio"
                name="mood-tagging-context-policy"
                checked={contextPolicy === "skip"}
                onChange={() => setContextPolicy("skip")}
              />
              <span>
                Skip incomplete tracks
                <span className="cleanup-hint muted">
                  Only tracks with complete current context are sent.
                </span>
              </span>
            </label>
          </div>
          <Link to="/assistant/context" onClick={onClose}>
            Open context analysis
          </Link>
        </section>
      ) : null}

      <label className="cleanup-choice mood-tagging-force">
        <input
          type="checkbox"
          checked={force}
          onChange={(event) => setForce(event.target.checked)}
        />
        <span>
          Rebuild current suggestions
          <span className="cleanup-hint muted">Normally only new or changed tracks are sent.</span>
        </span>
      </label>

      <p className="mood-tagging-boundary">
        Suggestions use the editable <Link to="/assistant/tags" onClick={onClose}>terrain, scene, and mood vocabulary</Link>.
        Accepted tags live only in the music database; album, year, genre, and other file metadata are never rewritten.
      </p>
    </div>
  );

  const runningBody = job === null ? null : (
    <div className="mood-tagging-running">
      <div className="assistant-job-progress">
        <div className="assistant-job-progress-label">
          <strong>{job.progress_phase || "Queued"}</strong>
          {job.progress_total !== null ? <span>{job.progress_current} / {job.progress_total}</span> : null}
        </div>
        {job.progress_total === null ? (
          <progress aria-label="Mood tagging progress" />
        ) : (
          <progress
            aria-label="Mood tagging progress"
            value={job.progress_current}
            max={Math.max(1, job.progress_total)}
          />
        )}
        {job.progress_message ? <p>{job.progress_message}</p> : null}
      </div>
      {job.status === "failed" ? (
        <p className="error">{job.error || "The provider did not complete this run."}</p>
      ) : job.status === "cancelled" ? (
        <p>The run was cancelled. Completed batches remain available for review.</p>
      ) : (
        <p className="muted small">You can close this window; the server-side run will continue.</p>
      )}
    </div>
  );

  const reviewBody = (
    <div className="mood-tagging-review-shell">
      {reviewError !== null ? <p className="error">{reviewError}</p> : null}
      <div className="cleanup-review-controls">
        <span>
          <strong>{page?.total ?? 0}</strong> track{page?.total === 1 ? "" : "s"} still need review
        </span>
        <span className="cleanup-review-spacer" />
        <button type="button" className="btn-ghost" onClick={() => selectVisible("all")}>All on page</button>
        <button type="button" className="btn-ghost" onClick={() => selectVisible("confident")}>High + medium</button>
        <button type="button" className="btn-ghost" onClick={() => selectVisible("none")}>None</button>
      </div>
      {page !== null && page.items.length === 0 ? (
        <EmptyState title="No model suggestions need review">
          The model returned no tags for this scope, or every suggestion has already been reviewed.
        </EmptyState>
      ) : (
        <div className="mood-tagging-review">
          {page?.items.map((track) => {
            const playing = ambientTrackId === track.track_id && ambientIsPlaying;
            return (
              <article className="mood-tagging-track" key={track.track_id}>
                <header className="mood-tagging-track-heading">
                  <button
                    type="button"
                    className="mood-tagging-play"
                    aria-label={`${playing ? "Pause" : "Play"} ${track.display_title || track.title}`}
                    title={playing ? "Pause audition" : "Audition track"}
                    onClick={() => audition(track.track_id)}
                  >
                    {playing ? <PauseIcon /> : <PlayIcon />}
                  </button>
                  <div>
                    <strong>{track.display_title || track.title || "Untitled track"}</strong>
                    <span>{track.artist || "Unknown artist"} · {track.path}</span>
                  </div>
                </header>
                {track.manual_tags.length > 0 ? (
                  <p className="mood-tagging-existing">
                    In mood library: {track.manual_tags.join(" · ")}
                  </p>
                ) : null}
                <div className="mood-tagging-suggestions">
                  {track.analysis_suggestions.map((suggestion) => {
                    const target = {
                      track_id: track.track_id,
                      tag: suggestion.tag,
                      analyzer_id: suggestion.analyzer_id,
                      source_signature: suggestion.source_signature,
                    };
                    const key = suggestionKey(target);
                    return (
                      <label className={`mood-tagging-suggestion is-${suggestion.confidence}`} key={key}>
                        <input
                          type="checkbox"
                          aria-label={`Select ${suggestion.tag} (${suggestion.confidence} confidence)`}
                          checked={selected.has(key)}
                          disabled={busy}
                          onChange={(event) => {
                            setSelected((current) => {
                              const next = new Set(current);
                              if (event.target.checked) next.add(key);
                              else next.delete(key);
                              return next;
                            });
                          }}
                        />
                        <span>{suggestion.tag}</span>
                        <small>{suggestion.confidence}</small>
                      </label>
                    );
                  })}
                </div>
                {track.analysis_suggestions[0]?.evidence.length ? (
                  <details className="mood-tagging-evidence">
                    <summary>Why these tags were suggested</summary>
                    <ul>
                      {track.analysis_suggestions[0].evidence.map((item) => <li key={item}>{item}</li>)}
                    </ul>
                  </details>
                ) : null}
              </article>
            );
          })}
        </div>
      )}
      {page !== null && page.total > PAGE_SIZE ? (
        <div className="mood-tagging-pagination">
          <button type="button" disabled={busy || offset === 0} onClick={() => void loadReview(Math.max(0, offset - PAGE_SIZE))}>Previous</button>
          <span>{offset + 1}–{Math.min(offset + page.items.length, page.total)} of {page.total}</span>
          <button type="button" disabled={busy || offset + PAGE_SIZE >= page.total} onClick={() => void loadReview(offset + PAGE_SIZE)}>Next</button>
        </div>
      ) : null}
    </div>
  );

  const result = modelTaggingResultFromJob(job);
  const doneBody = (
    <div className="mood-tagging-done">
      <strong>Suggestions are ready for review</strong>
      <p>
        {result === null
          ? "The run finished."
          : `${result.updated_profiles} profiles were updated and ${result.unchanged_profiles} were already current.`}
      </p>
    </div>
  );

  const footer =
    step === "configure" ? (
      <>
        <button type="button" onClick={onClose}>Cancel</button>
        <button
          type="button"
          className="btn-secondary"
          disabled={busy || planLoading || plan === null || plan.scope_tracks === 0}
          onClick={() => void loadReview(0, scope)}
        >
          Review existing suggestions
        </button>
        <button
          type="button"
          className="btn-primary"
          disabled={busy || planLoading || plan === null || !plan.available || plan.scope_tracks === 0}
          onClick={() => void startTagging()}
        >
          {busy
            ? "Starting…"
            : plan !== null && !force && plan.tracks_needing_tags === 0
              ? "Review current suggestions"
              : "Create suggestions"}
        </button>
      </>
    ) : step === "running" && job !== null && isModelTaggingJobActive(job) ? (
      <>
        <button type="button" onClick={onClose}>Close and keep running</button>
        <button type="button" className="btn-secondary" disabled={busy} onClick={() => void cancelJob()}>
          {job.status === "cancel_requested" ? "Cancelling…" : "Cancel run"}
        </button>
      </>
    ) : step === "running" ? (
      <>
        <button type="button" onClick={onClose}>Close</button>
        <button type="button" className="btn-primary" onClick={() => void loadReview(0)}>Review completed suggestions</button>
      </>
    ) : step === "review" ? (
      <>
        <button type="button" className="btn-ghost" onClick={() => setStep("configure")}>New run</button>
        <button type="button" onClick={onClose}>Close</button>
        <button type="button" className="btn-secondary" disabled={busy || selectedTargets.length === 0} onClick={() => void reviewSelected("rejected")}>Reject selected</button>
        <button type="button" className="btn-primary" disabled={busy || selectedTargets.length === 0} onClick={() => void reviewSelected("accepted")}>Add {selectedTargets.length || "selected"} to mood library</button>
      </>
    ) : (
      <button type="button" className="btn-primary" onClick={onClose}>Close</button>
    );

  const titles: Record<Step, string> = {
    configure: "Create mood tags",
    running: "Creating mood-tag suggestions",
    review: `Review mood tags — ${reviewScopeLabel}`,
    done: "Mood tagging complete",
  };

  return (
    <Modal
      title={titles[step]}
      ariaLabel="Create and review database mood tags"
      className="modal-mood-tagging"
      bodyClassName="mood-tagging-body"
      onClose={onClose}
      footer={footer}
      closeButton={step !== "running" || job === null || !isModelTaggingJobActive(job)}
    >
      {step === "configure"
        ? configureBody
        : step === "running"
          ? runningBody
          : step === "review"
            ? reviewBody
            : doneBody}
    </Modal>
  );
}
