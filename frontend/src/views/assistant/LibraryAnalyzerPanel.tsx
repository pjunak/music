import type {
  BackgroundJob,
  LibraryContextSummary,
} from "@/core/api";

import {
  analysisStatusLabel,
  formatAnalysisTime,
  isAnalysisJobActive,
} from "./analysisJobs";

function resultNumber(job: BackgroundJob, key: string): number | null {
  const value = job.result?.[key];
  return typeof value === "number" ? value : null;
}

interface AnalysisFailureSample {
  path: string;
  error: string;
}

function resultFailureSamples(job: BackgroundJob): AnalysisFailureSample[] {
  const value = job.result?.failure_samples;
  if (!Array.isArray(value)) return [];
  return value.flatMap((item) => {
    if (typeof item !== "object" || item === null) return [];
    const path = Reflect.get(item, "path");
    const error = Reflect.get(item, "error");
    return typeof path === "string" && typeof error === "string"
      ? [{ path, error }]
      : [];
  });
}

interface LibraryAnalyzerPanelProps {
  id: string;
  history: BackgroundJob[];
  summary: LibraryContextSummary | null;
  loading: boolean;
  actionBusy: boolean;
  progressLabel: string;
  emptyTitle: string;
  emptyDescription: string;
  analyzeLabel: string;
  checkLabel: string;
  rebuildTitle: string;
  onStart: (force: boolean) => void;
  onCancel: () => void;
  onRetry: () => void;
}

function analysisStateTitle(
  latest: BackgroundJob | undefined,
  active: boolean,
  currentTracks: number,
): string {
  if (active) return "Analysis in progress";
  if (latest?.status === "failed") return "Analysis needs attention";
  if (latest?.status === "cancelled") return "Analysis was cancelled";
  if (latest?.status === "succeeded" || currentTracks > 0) {
    return "Library context is ready";
  }
  return "Build library context";
}

export function LibraryAnalyzerPanel({
  id,
  history,
  summary,
  loading,
  actionBusy,
  progressLabel,
  emptyTitle,
  emptyDescription,
  analyzeLabel,
  checkLabel,
  rebuildTitle,
  onStart,
  onCancel,
  onRetry,
}: LibraryAnalyzerPanelProps) {
  const latest = history[0];
  const active = isAnalysisJobActive(latest);
  const currentTracks =
    summary === null ? 0 : summary.full_tracks + summary.partial_tracks;
  const coveragePercent =
    summary === null || summary.library_tracks === 0
      ? 0
      : Math.round((currentTracks / summary.library_tracks) * 100);
  const failed = latest === undefined ? null : resultNumber(latest, "failed");
  const failureSamples = latest === undefined ? [] : resultFailureSamples(latest);
  const stateTitle = analysisStateTitle(latest, active, currentTracks);

  return (
    <section
      className="surface-card assistant-context-analysis-card"
      aria-labelledby={`${id}-title`}
    >
      <div className="assistant-context-analysis-heading">
        <div>
          <h2 id={`${id}-title`}>{stateTitle}</h2>
          {summary !== null ? (
            <p>
              {currentTracks} of {summary.library_tracks} tracks have current local
              context.
            </p>
          ) : null}
        </div>
        {latest !== undefined ? (
          <span className={`assistant-job-status is-${latest.status}`}>
            {analysisStatusLabel(latest.status)}
          </span>
        ) : null}
      </div>

      {loading && latest === undefined ? (
        <p className="muted">Loading analysis history…</p>
      ) : latest === undefined ? (
        <div className="assistant-context-analysis-empty">
          <strong>{emptyTitle}</strong>
          <span>{emptyDescription}</span>
        </div>
      ) : (
        <div className="assistant-job-progress assistant-context-analysis-progress">
          <div className="assistant-job-progress-label">
            <strong>
              {latest.progress_phase || analysisStatusLabel(latest.status)}
            </strong>
            {latest.progress_total !== null ? (
              <span>
                {latest.progress_current} / {latest.progress_total}
              </span>
            ) : null}
          </div>
          {latest.progress_total === null ? (
            <progress aria-label={progressLabel} />
          ) : (
            <progress
              aria-label={progressLabel}
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
          {latest.status === "succeeded" && failed !== null && failed > 0 ? (
            <>
              <p className="error">
                {failed} {failed === 1 ? "track" : "tracks"} could not be decoded.
                A later check will retry them.
              </p>
              {failureSamples.length > 0 ? (
                <details className="assistant-job-failures">
                  <summary>Review failed tracks</summary>
                  <ul>
                    {failureSamples.map((sample) => (
                      <li key={`${sample.path}:${sample.error}`}>
                        <strong>{sample.path || "Unknown track"}</strong>
                        <span>{sample.error}</span>
                      </li>
                    ))}
                  </ul>
                </details>
              ) : null}
            </>
          ) : null}
        </div>
      )}

      {summary !== null ? (
        <div className="assistant-context-coverage-summary">
          <div className="assistant-context-coverage-heading">
            <div>
              <strong>Library coverage</strong>
              <span>{coveragePercent}% ready for tagging</span>
            </div>
          </div>
          <div
            className="assistant-context-coverage-meter"
            role="img"
            aria-label={`${summary.full_tracks} full, ${summary.partial_tracks} partial, ${summary.missing_tracks} missing or stale, and ${summary.failed_tracks} failed tracks`}
          >
            <span
              className="is-full"
              style={{
                width: `${(summary.full_tracks / Math.max(1, summary.library_tracks)) * 100}%`,
              }}
            />
            <span
              className="is-partial"
              style={{
                width: `${(summary.partial_tracks / Math.max(1, summary.library_tracks)) * 100}%`,
              }}
            />
            <span className="is-unavailable" />
          </div>
          <div className="assistant-context-coverage-stats">
            <div>
              <strong>{summary.full_tracks}</strong>
              <span>Full</span>
            </div>
            <div>
              <strong>{summary.partial_tracks}</strong>
              <span>Partial</span>
            </div>
            <div>
              <strong>{summary.missing_tracks}</strong>
              <span>Missing or stale</span>
            </div>
            <div>
              <strong>{summary.failed_tracks}</strong>
              <span>Failed</span>
            </div>
          </div>
          <p className="assistant-context-coverage-updated">
            Updated {formatAnalysisTime(summary.last_updated_at)}
          </p>
        </div>
      ) : null}

      <div className="assistant-analysis-actions">
        <button
          type="button"
          className="btn-primary"
          onClick={() => onStart(false)}
          disabled={active || actionBusy}
        >
          {latest?.status === "succeeded" ? checkLabel : analyzeLabel}
        </button>
        <button
          type="button"
          className="btn-ghost"
          onClick={() => onStart(true)}
          disabled={active || actionBusy}
          title={rebuildTitle}
        >
          Rebuild all profiles
        </button>
        {active ? (
          <button type="button" onClick={onCancel} disabled={actionBusy}>
            {latest?.status === "cancel_requested" ? "Cancelling…" : "Cancel"}
          </button>
        ) : null}
        {latest?.status === "failed" || latest?.status === "cancelled" ? (
          <button type="button" onClick={onRetry} disabled={actionBusy}>
            Retry last job
          </button>
        ) : null}
      </div>
    </section>
  );
}
