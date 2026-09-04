import { useEffect, useRef, useState } from "react";
import { Link } from "react-router-dom";

import { confirmDialog } from "@/components/confirmDialog";
import {
  type BackgroundJob,
  MODEL_TAG_CLEANUP_DISCLOSURE_VERSION,
  type ModelTagCleanupAvailability,
  assistantApi,
  jobsApi,
} from "@/core/api";
import { toast } from "@/core/toast";

import { readableBackgroundJobError } from "./backgroundJobs";
import { ProviderBoundaryPopover } from "./AssistantInfoPopover";
import {
  MODEL_TAG_CLEANUP_JOB_KIND,
  isModelTagCleanupJobActive,
  modelTagCleanupResultFromJob,
} from "./modelTagCleanupJobs";
import { ModelUsageSummary } from "./ModelUsageSummary";

interface Props {
  onCatalogChanged: () => void;
}

function errorMessage(error: unknown): string {
  return error instanceof Error
    ? error.message
    : "Mood-tag cleanup is unavailable.";
}

function unavailableMessage(reasonCode: string | null): string {
  switch (reasonCode) {
    case "model_quality_not_passed":
      return "Run and pass the mood-tag cleanup quality check in AI setup first.";
    case "role_not_enabled":
    case "role_not_configured":
      return "Choose and make a mood-tag cleanup model available in AI setup first.";
    case "connection_not_verified":
    case "model_not_tested":
      return "Verify and test the assigned mood-tag cleanup model in AI setup first.";
    case "tag_catalog_empty":
      return "Add at least one mood-library tag before asking a model to review the catalog.";
    case "tag_catalog_too_large":
      return "This model review currently supports at most 500 mood-library tags.";
    default:
      return "The connected mood-tag cleanup model is not ready yet.";
  }
}

export function ModelTagCleanupPanel({ onCatalogChanged }: Props) {
  const [availability, setAvailability] =
    useState<ModelTagCleanupAvailability | null>(null);
  const [job, setJob] = useState<BackgroundJob | null>(null);
  const [selectedIds, setSelectedIds] = useState<Set<string>>(() => new Set());
  const [loading, setLoading] = useState(true);
  const [loadError, setLoadError] = useState<string | null>(null);
  const [actionBusy, setActionBusy] = useState(false);
  const [refreshKey, setRefreshKey] = useState(0);
  const jobRef = useRef<BackgroundJob | null>(null);

  useEffect(() => {
    let disposed = false;
    let timer: number | undefined;

    async function poll(initial: boolean) {
      if (initial) setLoading(true);
      const [availabilityResult, historyResult] = await Promise.allSettled([
        assistantApi.getModelTagCleanupAvailability(),
        jobsApi.list({ kind: MODEL_TAG_CLEANUP_JOB_KIND, limit: 1 }),
      ]);
      if (disposed) return;

      const errors: string[] = [];
      if (availabilityResult.status === "fulfilled") {
        setAvailability(availabilityResult.value);
      } else {
        errors.push(errorMessage(availabilityResult.reason));
      }
      if (historyResult.status === "fulfilled") {
        const latest = historyResult.value[0] ?? null;
        if (jobRef.current?.id !== latest?.id) setSelectedIds(new Set());
        jobRef.current = latest;
        setJob(latest);
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
        isModelTagCleanupJobActive(latest) ? 1500 : 5000,
      );
    }

    void poll(true);
    return () => {
      disposed = true;
      if (timer !== undefined) window.clearTimeout(timer);
    };
  }, [refreshKey]);

  const active = isModelTagCleanupJobActive(job);
  const result = modelTagCleanupResultFromJob(job);
  const catalogIsCurrent =
    result !== null &&
    availability !== null &&
    result.catalog_signature === availability.catalog_signature &&
    result.vocabulary_fingerprint === availability.vocabulary_fingerprint;
  const selected =
    result?.suggestions.filter((item) => selectedIds.has(item.id)) ?? [];

  async function start() {
    if (availability === null || !availability.available) return;
    const confirmed = await confirmDialog({
      title: "Send your mood-tag catalog to the cleanup model?",
      body:
        `${availability.manual_tags} normalized mood-tag name${
          availability.manual_tags === 1 ? "" : "s"
        } will be checked against your controlled vocabulary. ` +
        (availability.estimated_provider_requests > 0
          ? `Only unresolved names, usage counts, and canonical tag IDs and definitions may be sent in ${availability.estimated_provider_requests} provider request${availability.estimated_provider_requests === 1 ? "" : "s"}. `
          : "Declared aliases and clear spelling rules resolve this catalog locally, so no provider request is expected. ") +
        "No songs, audio, titles, artists, albums, paths, playlists, generated tags, review history, or credentials will be sent. " +
        "The model can only return a proposal for you to review. Provider usage may incur cost.",
      confirmLabel: "Request cleanup suggestions",
      tone: "primary",
    });
    if (!confirmed) return;
    setActionBusy(true);
    try {
      const nextJob = await assistantApi.startModelTagCleanup(
        MODEL_TAG_CLEANUP_DISCLOSURE_VERSION,
      );
      jobRef.current = nextJob;
      setJob(nextJob);
      setSelectedIds(new Set());
      toast.success(
        "Mood-tag cleanup queued",
        "You can close this page; the proposal and progress are stored on the server.",
      );
      setRefreshKey((value) => value + 1);
    } catch (error) {
      toast.error("Mood-tag cleanup could not start", errorMessage(error));
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

  function toggleSuggestion(id: string) {
    setSelectedIds((current) => {
      const next = new Set(current);
      if (next.has(id)) next.delete(id);
      else next.add(id);
      return next;
    });
  }

  async function applySelected() {
    if (job === null || result === null || !catalogIsCurrent || selected.length === 0) {
      return;
    }
    const confirmed = await confirmDialog({
      title: `Apply ${selected.length} selected tag rename${
        selected.length === 1 ? "" : "s"
      }?`,
      body:
        selected.map((item) => `${item.source} → ${item.target}`).join("; ") +
        ". This changes only the selected database mood tags. Unselected suggestions remain untouched.",
      confirmLabel: "Apply selected renames",
      tone: "primary",
    });
    if (!confirmed) return;
    setActionBusy(true);
    try {
      const outcome = await assistantApi.applyModelTagCleanup(
        job.id,
        result.catalog_signature,
        result.vocabulary_fingerprint,
        selected.map(({ source, target }) => ({ source, target })),
      );
      setSelectedIds(new Set());
      onCatalogChanged();
      toast.success(
        "Selected tag renames applied",
        `${outcome.applied.length} rename${
          outcome.applied.length === 1 ? " was" : "s were"
        } applied atomically.`,
      );
      setRefreshKey((value) => value + 1);
    } catch (error) {
      toast.error("Selected tag renames were not applied", errorMessage(error));
      setRefreshKey((value) => value + 1);
    } finally {
      setActionBusy(false);
    }
  }

  return (
    <section
      className="surface-card assistant-model-tagging assistant-model-tag-cleanup"
      aria-label="Connected model mood-tag cleanup"
    >
      <div className="assistant-analyzer-heading">
        <div>
          <p className="assistant-eyebrow">Optional connected model</p>
          <h2>Review mood-tag consistency</h2>
          <p>
            Ask the independently assigned cleanup model to propose duplicate,
            typo, or inconsistent tag renames. Nothing changes until you select
            and apply individual suggestions.
          </p>
        </div>
        <span
          className={`assistant-job-status is-${
            active
              ? job?.status
              : job?.status === "failed" || job?.status === "cancelled"
                ? job.status
                : availability?.available
                  ? "succeeded"
                  : "queued"
          }`}
        >
          {active
            ? job?.status === "cancel_requested"
              ? "Cancelling"
              : "Running"
            : job?.status === "failed"
              ? "Last run failed"
              : job?.status === "cancelled"
                ? "Last run cancelled"
                : availability?.available
                  ? "Ready"
                  : "Optional"}
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
        <p className="muted">Checking the mood-tag cleanup model…</p>
      ) : null}
      {availability !== null && !availability.available && !active ? (
        <div className="assistant-model-tagging-unavailable">
          <strong>Model mood-tag cleanup is not ready</strong>
          <p>{unavailableMessage(availability.reason_code)}</p>
          {availability.reason_code !== "tag_catalog_empty" &&
          availability.reason_code !== "tag_catalog_too_large" ? (
            <Link to="/assistant/ai">Open AI setup</Link>
          ) : null}
        </div>
      ) : null}

      {availability !== null && (availability.available || active) ? (
        <ProviderBoundaryPopover
          shared={availability.disclosure.shared_with_provider}
          neverShared={availability.disclosure.never_shared}
          footer={
            <>
              {availability.connection_name ?? "Provider"} ·{" "}
              {availability.model_id ?? "assigned model"} · {availability.manual_tags}{" "}
              mood-library tags · at most {availability.disclosure.maximum_tags} tags
              per review.
            </>
          }
        />
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
            <progress aria-label="Model mood-tag cleanup progress" />
          ) : (
            <progress
              aria-label="Model mood-tag cleanup progress"
              value={job.progress_current}
              max={Math.max(1, job.progress_total)}
            />
          )}
          {job.progress_message ? <p>{job.progress_message}</p> : null}
          <p>Safe to close: this server-side run will keep going.</p>
        </div>
      ) : job?.status === "failed" ? (
        <div className="assistant-quality-result is-failed">
          <strong>The model mood-tag cleanup did not finish</strong>
          <p>
            {readableBackgroundJobError(
              job.error,
              "Start a new review when the provider is available.",
            )}
          </p>
        </div>
      ) : job?.status === "cancelled" ? (
        <div className="assistant-quality-result">
          <strong>The model mood-tag cleanup was cancelled</strong>
          <p>No mood-library tags were changed.</p>
        </div>
      ) : null}

      {job?.status === "succeeded" && result === null ? (
        <div className="assistant-quality-result is-failed" role="alert">
          <strong>The stored cleanup proposal could not be read</strong>
          <p>Run a new review before applying any tag changes.</p>
        </div>
      ) : null}
      {result !== null ? (
        <div className="assistant-model-tag-cleanup-review">
          <div className="assistant-tag-cleanup-heading">
            <div>
              <strong>Review-only proposal</strong>
              <span>
                {result.suggestions.length} suggestion
                {result.suggestions.length === 1 ? "" : "s"}; none selected
                automatically
              </span>
            </div>
          </div>
          {!catalogIsCurrent ? (
            <div className="assistant-analysis-error" role="alert">
              <span>
                The mood-tag catalog or controlled vocabulary changed after this
                proposal was created. Run a new review; this proposal cannot be
                applied.
              </span>
            </div>
          ) : result.suggestions.length === 0 ? (
            <p className="muted">
              The model found no safe consistency changes to suggest.
            </p>
          ) : (
            <div className="assistant-model-tag-cleanup-list">
              {result.suggestions.map((suggestion) => (
                <label key={suggestion.id}>
                  <input
                    type="checkbox"
                    checked={selectedIds.has(suggestion.id)}
                    disabled={actionBusy || !catalogIsCurrent}
                    onChange={() => toggleSuggestion(suggestion.id)}
                    aria-label={`Select ${suggestion.source} to ${suggestion.target}`}
                  />
                  <span>
                    <strong>
                      {suggestion.source} → {suggestion.target}
                    </strong>
                    <small>
                      {suggestion.origin === "local-rule"
                        ? "local rule"
                        : "model"}{" "}
                      · {suggestion.confidence} confidence · {suggestion.reason}
                    </small>
                    <small>
                      {suggestion.source_track_count} source track
                      {suggestion.source_track_count === 1 ? "" : "s"}
                      {suggestion.merged
                        ? ` · merges into ${suggestion.target_track_count} existing track${
                            suggestion.target_track_count === 1 ? "" : "s"
                          }`
                        : " · renames to a new catalog tag"}
                    </small>
                  </span>
                </label>
              ))}
            </div>
          )}
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
            {job.status === "cancel_requested" ? "Cancelling…" : "Cancel review"}
          </button>
        </div>
      ) : (
        <div className="assistant-model-tagging-actions">
          {result !== null && result.suggestions.length > 0 ? (
            <button
              type="button"
              className="btn-primary"
              disabled={actionBusy || !catalogIsCurrent || selected.length === 0}
              onClick={() => void applySelected()}
            >
              {actionBusy
                ? "Applying…"
                : `Apply ${selected.length} selected rename${
                    selected.length === 1 ? "" : "s"
                  }`}
            </button>
          ) : null}
          {availability?.available ? (
            <button
              type="button"
              className={result === null ? "btn-primary" : "btn-secondary"}
              disabled={actionBusy}
              onClick={() => void start()}
            >
              {actionBusy
                ? "Starting…"
                : result === null
                  ? `Review ${availability.manual_tags} mood-library tags`
                  : "Run a new review"}
            </button>
          ) : null}
        </div>
      )}
    </section>
  );
}
