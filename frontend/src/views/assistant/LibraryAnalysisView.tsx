import { lazy, Suspense, useEffect, useMemo, useState } from "react";

import { EmptyState } from "@/components/EmptyState";
import {
  type BackgroundJob,
  type BackgroundJobStatus,
  type LibraryAnalysisSummary,
  assistantApi,
  jobsApi,
} from "@/core/api";
import { toast } from "@/core/toast";

const LibraryTagEditor = lazy(async () => {
  const module = await import("./LibraryTagEditor");
  return { default: module.LibraryTagEditor };
});

const ANALYSIS_JOB_KIND = "assistant.library-analysis";
const ACTIVE_STATUSES = new Set<BackgroundJobStatus>([
  "queued",
  "running",
  "cancel_requested",
]);

function isActive(job: BackgroundJob | undefined): boolean {
  return job !== undefined && ACTIVE_STATUSES.has(job.status);
}

function statusLabel(status: BackgroundJobStatus): string {
  return status.replace("_", " ");
}

function formatTime(value: string | null): string {
  if (value === null) return "Never";
  return new Intl.DateTimeFormat(undefined, {
    dateStyle: "medium",
    timeStyle: "short",
  }).format(new Date(value));
}

function resultNumber(job: BackgroundJob, key: string): number | null {
  const value = job.result?.[key];
  return typeof value === "number" ? value : null;
}

export function LibraryAnalysisView() {
  const [history, setHistory] = useState<BackgroundJob[]>([]);
  const [summary, setSummary] = useState<LibraryAnalysisSummary | null>(null);
  const [loading, setLoading] = useState(true);
  const [loadError, setLoadError] = useState<string | null>(null);
  const [actionBusy, setActionBusy] = useState(false);
  const [refreshKey, setRefreshKey] = useState(0);

  const latest = history[0];
  const active = isActive(latest);

  useEffect(() => {
    let disposed = false;
    let timer: number | undefined;

    async function poll(initial: boolean) {
      if (initial) setLoading(true);
      try {
        const [jobs, analysisSummary] = await Promise.all([
          jobsApi.list({ kind: ANALYSIS_JOB_KIND, limit: 10 }),
          assistantApi.getLibraryAnalysisSummary(),
        ]);
        if (disposed) return;
        setHistory(jobs);
        setSummary(analysisSummary);
        setLoadError(null);
        timer = window.setTimeout(
          () => void poll(false),
          isActive(jobs[0]) ? 1500 : 5000,
        );
      } catch (error) {
        if (disposed) return;
        setLoadError(
          error instanceof Error ? error.message : "Analysis status is unavailable.",
        );
        timer = window.setTimeout(() => void poll(false), 5000);
      } finally {
        if (!disposed) setLoading(false);
      }
    }

    void poll(true);
    return () => {
      disposed = true;
      if (timer !== undefined) window.clearTimeout(timer);
    };
  }, [refreshKey]);

  const confidenceTotal = useMemo(
    () =>
      summary === null
        ? 0
        : summary.high_confidence +
          summary.medium_confidence +
          summary.low_confidence,
    [summary],
  );

  async function start(force: boolean) {
    setActionBusy(true);
    try {
      const job = await assistantApi.startLibraryAnalysis(force);
      setHistory((current) => [job, ...current.filter((item) => item.id !== job.id)]);
      toast.success(
        force ? "Library rebuild queued" : "Library analysis queued",
        "You can leave this page; progress is stored on the server.",
      );
      setRefreshKey((value) => value + 1);
    } catch (error) {
      toast.error(
        "Analysis could not start",
        error instanceof Error ? error.message : undefined,
      );
    } finally {
      setActionBusy(false);
    }
  }

  async function cancel() {
    if (latest === undefined) return;
    setActionBusy(true);
    try {
      const job = await jobsApi.cancel(latest.id);
      setHistory((current) => [job, ...current.filter((item) => item.id !== job.id)]);
      setRefreshKey((value) => value + 1);
    } catch (error) {
      toast.error(
        "Cancellation failed",
        error instanceof Error ? error.message : undefined,
      );
    } finally {
      setActionBusy(false);
    }
  }

  async function retry() {
    if (latest === undefined) return;
    setActionBusy(true);
    try {
      const job = await jobsApi.retry(latest.id);
      setHistory((current) => [job, ...current]);
      setRefreshKey((value) => value + 1);
    } catch (error) {
      toast.error("Retry failed", error instanceof Error ? error.message : undefined);
    } finally {
      setActionBusy(false);
    }
  }

  return (
    <div className="assistant-analysis-view">
      <header className="assistant-page-header assistant-analysis-header">
        <div>
          <p className="assistant-eyebrow">Durable server-side work</p>
          <h1>Library analysis</h1>
          <p>
            Build reusable mood profiles for the whole library. Jobs continue on
            the server and this page restores their progress after refresh or reopen.
          </p>
        </div>
        <span className="assistant-algorithm">local-metadata/v1</span>
      </header>

      {loadError !== null ? (
        <div className="assistant-analysis-error" role="alert">
          <span>{loadError}</span>
          <button type="button" onClick={() => setRefreshKey((value) => value + 1)}>
            Retry
          </button>
        </div>
      ) : null}

      <div className="assistant-analysis-grid">
        <section className="surface-card authoring-card assistant-analysis-current">
          <div className="assistant-section-heading">
            <div>
              <p className="assistant-eyebrow">Current work</p>
              <h2>{active ? "Analysis in progress" : "Analysis is ready"}</h2>
            </div>
            {latest !== undefined ? (
              <span className={`assistant-job-status is-${latest.status}`}>
                {statusLabel(latest.status)}
              </span>
            ) : null}
          </div>

          {loading && latest === undefined ? (
            <p className="muted">Loading analysis history…</p>
          ) : latest === undefined ? (
            <EmptyState title="No library analysis has run yet">
              Start the local pass to turn existing metadata and BPM values into
              reusable, versioned mood profiles.
            </EmptyState>
          ) : (
            <div className="assistant-job-progress">
              <div className="assistant-job-progress-label">
                <strong>{latest.progress_phase || statusLabel(latest.status)}</strong>
                {latest.progress_total !== null ? (
                  <span>
                    {latest.progress_current} / {latest.progress_total}
                  </span>
                ) : null}
              </div>
              {latest.progress_total === null ? (
                <progress aria-label="Library analysis progress" />
              ) : (
                <progress
                  aria-label="Library analysis progress"
                  value={latest.progress_current}
                  max={Math.max(1, latest.progress_total)}
                />
              )}
              {latest.progress_message ? <p>{latest.progress_message}</p> : null}
              {latest.error ? <p className="error">{latest.error}</p> : null}
              {latest.status === "succeeded" ? (
                <p>
                  {resultNumber(latest, "updated") ?? 0} profiles updated ·{" "}
                  {resultNumber(latest, "unchanged") ?? 0} already current
                </p>
              ) : null}
            </div>
          )}

          <div className="assistant-analysis-actions">
            <button
              type="button"
              className="btn-primary"
              onClick={() => void start(false)}
              disabled={active || actionBusy}
            >
              {latest?.status === "succeeded" ? "Check for changes" : "Analyze library"}
            </button>
            <button
              type="button"
              className="btn-ghost"
              onClick={() => void start(true)}
              disabled={active || actionBusy}
              title="Recompute every profile even when its source metadata is unchanged"
            >
              Rebuild all profiles
            </button>
            {active ? (
              <button type="button" onClick={() => void cancel()} disabled={actionBusy}>
                {latest?.status === "cancel_requested" ? "Cancelling…" : "Cancel"}
              </button>
            ) : null}
            {latest?.status === "failed" || latest?.status === "cancelled" ? (
              <button type="button" onClick={() => void retry()} disabled={actionBusy}>
                Retry last job
              </button>
            ) : null}
          </div>
        </section>

        <section className="surface-card assistant-analysis-summary">
          <div className="assistant-section-heading">
            <div>
              <p className="assistant-eyebrow">Stored profiles</p>
              <h2>Coverage</h2>
            </div>
            <span>{summary?.analyzer ?? "local-metadata/v1"}</span>
          </div>
          <div className="assistant-analysis-stats">
            <div>
              <strong>{summary?.analyzed_tracks ?? 0}</strong>
              <span>Analyzed</span>
            </div>
            <div>
              <strong>{summary?.library_tracks ?? 0}</strong>
              <span>Library tracks</span>
            </div>
            <div>
              <strong>{summary?.high_confidence ?? 0}</strong>
              <span>High confidence</span>
            </div>
            <div>
              <strong>{summary?.low_confidence ?? 0}</strong>
              <span>Needs richer data</span>
            </div>
          </div>
          {summary !== null && summary.library_tracks > 0 ? (
            <div className="assistant-analysis-coverage">
              <span
                style={{
                  width: `${Math.round((confidenceTotal / summary.library_tracks) * 100)}%`,
                }}
              />
            </div>
          ) : null}
          <p className="assistant-analysis-note">
            This pass uses existing metadata only. It does not identify vocals,
            instruments, key, loudness, or spectral character from the audio signal.
          </p>
          <p className="muted small">
            Last updated: {formatTime(summary?.last_updated_at ?? null)}
          </p>
        </section>
      </div>

      <Suspense
        fallback={
          <section className="surface-card assistant-tag-workspace">
            <p className="muted">Loading manual tag editor…</p>
          </section>
        }
      >
        <LibraryTagEditor />
      </Suspense>

      <section className="surface-card assistant-analysis-history">
        <div className="assistant-section-heading">
          <div>
            <p className="assistant-eyebrow">Persistent history</p>
            <h2>Recent jobs</h2>
          </div>
        </div>
        {history.length === 0 ? (
          <p className="muted">Completed and interrupted runs will appear here.</p>
        ) : (
          <div className="assistant-job-history-list">
            {history.map((job) => (
              <div className="assistant-job-history-row" key={job.id}>
                <span className={`assistant-job-status is-${job.status}`}>
                  {statusLabel(job.status)}
                </span>
                <span>{job.progress_phase || "Queued"}</span>
                <span>{formatTime(job.created_at)}</span>
                <span>Attempt {job.attempts || 1}</span>
              </div>
            ))}
          </div>
        )}
      </section>
    </div>
  );
}
