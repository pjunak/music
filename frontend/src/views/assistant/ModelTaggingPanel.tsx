import { useEffect, useMemo, useRef, useState } from "react";
import { Link } from "react-router-dom";

import { confirmDialog } from "@/components/confirmDialog";
import {
  type BackgroundJob,
  MODEL_TAGGING_DISCLOSURE_VERSION,
  type ModelTaggingAvailability,
  assistantApi,
  jobsApi,
} from "@/core/api";
import { toast } from "@/core/toast";

import { readableBackgroundJobError } from "./backgroundJobs";
import {
  MODEL_TAGGING_JOB_KIND,
  isModelTaggingJobActive,
  modelTaggingResultFromJob,
} from "./modelTaggingJobs";
import { ModelUsageSummary } from "./ModelUsageSummary";

interface Props {
  onSuggestionsChanged: () => void;
}

function errorMessage(error: unknown): string {
  return error instanceof Error ? error.message : "Model tagging is unavailable.";
}

function unavailableMessage(reasonCode: string | null): string {
  switch (reasonCode) {
    case "model_quality_not_passed":
      return "Run and pass the music tagging quality check in AI Setup first.";
    case "role_not_enabled":
    case "role_not_configured":
      return "Assign and enable a music tagging model in AI Setup first.";
    case "connection_not_verified":
    case "model_not_tested":
      return "Verify and test the assigned music tagging model in AI Setup first.";
    default:
      return "The connected music tagging model is not ready yet.";
  }
}

export function ModelTaggingPanel({ onSuggestionsChanged }: Props) {
  const [availability, setAvailability] =
    useState<ModelTaggingAvailability | null>(null);
  const [job, setJob] = useState<BackgroundJob | null>(null);
  const [loading, setLoading] = useState(true);
  const [loadError, setLoadError] = useState<string | null>(null);
  const [actionBusy, setActionBusy] = useState(false);
  const [force, setForce] = useState(false);
  const [refreshKey, setRefreshKey] = useState(0);
  const notifiedJobId = useRef<string | null>(null);
  const jobRef = useRef<BackgroundJob | null>(null);

  useEffect(() => {
    let disposed = false;
    let timer: number | undefined;

    async function poll(initial: boolean) {
      if (initial) setLoading(true);
      const [availabilityResult, historyResult] = await Promise.allSettled([
        assistantApi.getModelTaggingAvailability(),
        jobsApi.list({ kind: MODEL_TAGGING_JOB_KIND, limit: 1 }),
      ]);
      if (disposed) return;

      const errors: string[] = [];
      if (availabilityResult.status === "fulfilled") {
        setAvailability(availabilityResult.value);
      } else {
        errors.push(errorMessage(availabilityResult.reason));
      }
      if (historyResult.status === "fulfilled") {
        const latestJob = historyResult.value[0] ?? null;
        jobRef.current = latestJob;
        setJob(latestJob);
      } else {
        errors.push(errorMessage(historyResult.reason));
      }
      setLoadError(errors.length > 0 ? [...new Set(errors)].join(" ") : null);
      setLoading(false);
      const latest =
        historyResult.status === "fulfilled"
          ? historyResult.value[0]
          : jobRef.current;
      timer = window.setTimeout(
        () => void poll(false),
        isModelTaggingJobActive(latest) ? 1500 : 5000,
      );
    }

    void poll(true);
    return () => {
      disposed = true;
      if (timer !== undefined) window.clearTimeout(timer);
    };
  }, [refreshKey]);

  useEffect(() => {
    if (job?.status !== "succeeded" || notifiedJobId.current === job.id) return;
    notifiedJobId.current = job.id;
    onSuggestionsChanged();
  }, [job, onSuggestionsChanged]);

  const active = isModelTaggingJobActive(job);
  const result = modelTaggingResultFromJob(job);
  const headingStatusClass = active && job !== null
    ? job.status
    : job?.status === "failed" || job?.status === "cancelled"
      ? job.status
      : availability?.available
        ? "succeeded"
        : "queued";
  const headingStatusLabel = active && job !== null
    ? job.status === "cancel_requested"
      ? "Cancelling"
      : "Running"
    : job?.status === "failed"
      ? "Last run failed"
      : job?.status === "cancelled"
        ? "Last run cancelled"
        : availability?.available
          ? "Ready"
          : "Optional";
  const requestPlan = useMemo(() => {
    if (availability === null) return { tracks: 0, requests: 0 };
    if (!force) {
      return {
        tracks: availability.tracks_needing_tags,
        requests: availability.estimated_provider_requests,
      };
    }
    return {
      tracks: availability.library_tracks,
      requests: Math.ceil(
        availability.library_tracks /
          Math.max(1, availability.disclosure.tracks_per_request),
      ),
    };
  }, [availability, force]);

  async function start() {
    if (availability === null || !availability.available) return;
    const confirmed = await confirmDialog({
      title: "Send library metadata to your tagging model?",
      body:
        `${requestPlan.tracks} track${requestPlan.tracks === 1 ? "" : "s"} will be ` +
        `processed in about ${requestPlan.requests} provider request${
          requestPlan.requests === 1 ? "" : "s"
        }. Indexed titles, artists, albums, origins, genres, durations, and BPM ` +
        "may be sent with a numeric matching ID and the fixed vocabulary. " +
        "Audio, paths, manual tags, local suggestions, and review decisions stay on this server. Provider usage may incur cost.",
      confirmLabel: requestPlan.tracks === 0 ? "Check current tags" : "Suggest tags",
      tone: "primary",
    });
    if (!confirmed) return;
    setActionBusy(true);
    try {
      const nextJob = await assistantApi.startModelTagging(
        force,
        MODEL_TAGGING_DISCLOSURE_VERSION,
      );
      jobRef.current = nextJob;
      setJob(nextJob);
      toast.success(
        "Model tagging queued",
        "You can close this page; progress and completed suggestions are stored on the server.",
      );
      setRefreshKey((value) => value + 1);
    } catch (error) {
      toast.error("Model tagging could not start", errorMessage(error));
    } finally {
      setActionBusy(false);
    }
  }

  async function cancel() {
    if (job === null || !active) return;
    setActionBusy(true);
    try {
      const nextJob = await jobsApi.cancel(job.id);
      jobRef.current = nextJob;
      setJob(nextJob);
      setRefreshKey((value) => value + 1);
    } catch (error) {
      toast.error("Cancellation failed", errorMessage(error));
    } finally {
      setActionBusy(false);
    }
  }

  return (
    <section
      className="surface-card assistant-model-tagging"
      aria-label="Connected model tag suggestions"
    >
      <div className="assistant-analyzer-heading">
        <div>
          <p className="assistant-eyebrow">Optional connected model</p>
          <h2>Suggest D&amp;D tags from metadata</h2>
          <p>
            Add a second, clearly labelled source of suggestions. Results remain
            generated evidence until you accept individual tags below.
          </p>
        </div>
        <span className={`assistant-job-status is-${headingStatusClass}`}>
          {headingStatusLabel}
        </span>
      </div>

      {loadError !== null ? (
        <div className="assistant-analysis-error" role="alert">
          <span>{loadError}</span>
          <button type="button" onClick={() => setRefreshKey((value) => value + 1)}>
            Retry
          </button>
        </div>
      ) : null}
      {loading && availability === null && job === null ? (
        <p className="muted">Checking the music tagging model…</p>
      ) : null}
      {availability !== null && !availability.available && !active ? (
        <div className="assistant-model-tagging-unavailable">
          <strong>Model tagging is not ready</strong>
          <p>{unavailableMessage(availability.reason_code)}</p>
          <Link to="/assistant/ai">Open AI Setup</Link>
        </div>
      ) : null}
      {availability !== null && (availability.available || active) ? (
        <>
          <div className="assistant-model-tagging-stats">
            <div>
              <strong>{availability.current_profiles}</strong>
              <span>Current model profiles</span>
            </div>
            <div>
              <strong>{availability.tracks_needing_tags}</strong>
              <span>Tracks needing suggestions</span>
            </div>
            <div>
              <strong>{availability.estimated_provider_requests}</strong>
              <span>Estimated requests</span>
            </div>
          </div>

          <div
            className="assistant-model-disclosure assistant-model-tagging-disclosure"
            aria-label="Music tagging model data disclosure"
          >
            <div className="assistant-model-disclosure-heading">
              <div>
                <span>Provider boundary</span>
                <strong>Review what leaves the server</strong>
              </div>
              <span>quality checked</span>
            </div>
            <div className="assistant-model-disclosure-grid">
              <div>
                <strong>Shared after confirmation</strong>
                <ul>
                  {availability.disclosure.shared_with_provider.map((item) => (
                    <li key={item}>{item}</li>
                  ))}
                </ul>
              </div>
              <div>
                <strong>Stays here</strong>
                <ul>
                  {availability.disclosure.never_shared.map((item) => (
                    <li key={item}>{item}</li>
                  ))}
                </ul>
              </div>
            </div>
            <details className="assistant-model-tagging-vocabulary">
              <summary>
                Review the {availability.disclosure.allowed_tags.length}-tag D&amp;D
                vocabulary
              </summary>
              <p>{availability.disclosure.allowed_tags.join(" · ")}</p>
            </details>
            <p>
              {availability.connection_name ?? "Provider"} ·{" "}
              {availability.model_id ?? "assigned model"} · up to{" "}
              {availability.disclosure.tracks_per_request} tracks per request.
            </p>
          </div>
        </>
      ) : null}

      {active && job !== null ? (
        <div className="assistant-job-progress assistant-model-tagging-progress">
          <div className="assistant-job-progress-label">
            <strong>{job.progress_phase || "Queued"}</strong>
            {job.progress_total !== null ? (
              <span>
                {job.progress_current} / {job.progress_total}
              </span>
            ) : null}
          </div>
          {job.progress_total === null ? (
            <progress aria-label="Model music tagging progress" />
          ) : (
            <progress
              aria-label="Model music tagging progress"
              value={job.progress_current}
              max={Math.max(1, job.progress_total)}
            />
          )}
          {job.progress_message ? <p>{job.progress_message}</p> : null}
          <p>Safe to close: this server-side run will keep going.</p>
        </div>
      ) : job?.status === "succeeded" ? (
        <div className="assistant-quality-result">
          <strong>Generated suggestions are ready for review</strong>
          {result === null ? (
            <p>The latest model tagging run completed.</p>
          ) : (
            <p>
              Updated {result.updated_profiles} profiles; {result.unchanged_profiles}{" "}
              were already current
              {result.skipped_changed_tracks > 0
                ? `; ${result.skipped_changed_tracks} changed during the run and were skipped`
                : ""}
              .
            </p>
          )}
        </div>
      ) : job?.status === "failed" ? (
        <div className="assistant-quality-result is-failed">
          <strong>The model tagging run did not finish</strong>
          <p>
            {readableBackgroundJobError(
              job.error,
              "Start a new run when the provider is available.",
            )}
          </p>
        </div>
      ) : job?.status === "cancelled" ? (
        <div className="assistant-quality-result">
          <strong>The model tagging run was cancelled</strong>
          <p>Completed batches remain available for review.</p>
        </div>
      ) : null}

      <ModelUsageSummary job={job} />

      {active && job !== null ? (
        <div className="assistant-model-tagging-actions">
          <button
            type="button"
            className="btn-secondary"
            disabled={actionBusy || job.status === "cancel_requested"}
            onClick={() => void cancel()}
          >
            {job.status === "cancel_requested" ? "Cancelling…" : "Cancel run"}
          </button>
        </div>
      ) : availability?.available ? (
        <>
          <div className="assistant-model-tagging-actions">
            <label className="checkbox-row">
              <input
                type="checkbox"
                checked={force}
                disabled={actionBusy}
                onChange={(event) => setForce(event.target.checked)}
              />
              <span>Rebuild every model profile</span>
            </label>
            <button
              type="button"
              className="btn-primary"
              disabled={actionBusy}
              onClick={() => void start()}
            >
              {actionBusy
                ? "Starting…"
                : requestPlan.tracks === 0
                  ? "Check for metadata changes"
                  : `Suggest tags for ${requestPlan.tracks} tracks`}
            </button>
          </div>
          <p className="assistant-model-tagging-estimate">
            Planned run: about {requestPlan.requests} provider request
            {requestPlan.requests === 1 ? "" : "s"}. Nothing is sent until you
            confirm.
          </p>
        </>
      ) : null}
    </section>
  );
}
