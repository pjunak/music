import { useMemo } from "react";

import { EmptyState } from "@/components/EmptyState";
import type {
  BackgroundJob,
  LibraryAnalysisSummary,
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
  title: string;
  description: string;
  analyzer: string;
  history: BackgroundJob[];
  summary: LibraryAnalysisSummary | null;
  loading: boolean;
  actionBusy: boolean;
  progressLabel: string;
  emptyTitle: string;
  emptyDescription: string;
  analyzeLabel: string;
  checkLabel: string;
  rebuildTitle: string;
  coverageNote: string;
  showFailureStat: boolean;
  onStart: (force: boolean) => void;
  onCancel: () => void;
  onRetry: () => void;
}

export function LibraryAnalyzerPanel({
  id,
  title,
  description,
  analyzer,
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
  coverageNote,
  showFailureStat,
  onStart,
  onCancel,
  onRetry,
}: LibraryAnalyzerPanelProps) {
  const latest = history[0];
  const active = isAnalysisJobActive(latest);
  const confidenceTotal = useMemo(
    () =>
      summary === null
        ? 0
        : summary.high_confidence +
          summary.medium_confidence +
          summary.low_confidence,
    [summary],
  );
  const failed = latest === undefined ? null : resultNumber(latest, "failed");
  const failureSamples = latest === undefined ? [] : resultFailureSamples(latest);

  return (
    <section className="assistant-analyzer-section" aria-labelledby={`${id}-title`}>
      <div className="assistant-analyzer-heading">
        <div>
          <p className="assistant-eyebrow">Local analyzer</p>
          <h2 id={`${id}-title`}>{title}</h2>
          <p>{description}</p>
        </div>
        <span className="assistant-algorithm">{analyzer}</span>
      </div>

      <div className="assistant-analysis-grid">
        <div className="surface-card authoring-card assistant-analysis-current">
          <div className="assistant-section-heading">
            <div>
              <p className="assistant-eyebrow">Current work</p>
              <h3>{active ? "Analysis in progress" : "Analysis is ready"}</h3>
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
            <EmptyState title={emptyTitle}>{emptyDescription}</EmptyState>
          ) : (
            <div className="assistant-job-progress">
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
                    {failed} {failed === 1 ? "track" : "tracks"} could not be
                    decoded. A later check will retry them.
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
        </div>

        <div className="surface-card assistant-analysis-summary">
          <div className="assistant-section-heading">
            <div>
              <p className="assistant-eyebrow">Stored profiles</p>
              <h3>Coverage</h3>
            </div>
            <span>{summary?.analyzer ?? analyzer}</span>
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
              <strong>
                {showFailureStat
                  ? (summary?.failed_tracks ?? 0)
                  : (summary?.low_confidence ?? 0)}
              </strong>
              <span>{showFailureStat ? "Failed tracks" : "Needs richer data"}</span>
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
          {summary !== null && summary.stale_tracks > 0 ? (
            <p className="muted small">
              {summary.stale_tracks} stale{" "}
              {summary.stale_tracks === 1 ? "profile needs" : "profiles need"} a
              refresh.
            </p>
          ) : null}
          <p className="assistant-analysis-note">{coverageNote}</p>
          <p className="muted small">
            Last updated: {formatAnalysisTime(summary?.last_updated_at ?? null)}
          </p>
        </div>
      </div>
    </section>
  );
}
