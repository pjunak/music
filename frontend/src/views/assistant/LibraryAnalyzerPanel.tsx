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

interface AnalysisPerformance {
  tracksProfiled: number;
  wallSeconds: number;
  realtimeFactor: number | null;
  dominantStage: string | null;
  stages: Array<{ id: string; seconds: number; sharePercent: number | null }>;
}

function numberProperty(value: object, key: string): number | null {
  const property = Reflect.get(value, key);
  return typeof property === "number" && Number.isFinite(property) ? property : null;
}

function analysisPerformance(job: BackgroundJob): AnalysisPerformance | null {
  const value = job.result?.performance;
  if (typeof value !== "object" || value === null) return null;
  const tracksProfiled = numberProperty(value, "tracks_profiled");
  const wallSeconds = numberProperty(value, "wall_seconds");
  const rawStages = Reflect.get(value, "stage_seconds");
  const rawShares = Reflect.get(value, "stage_share_percent");
  if (
    tracksProfiled === null ||
    wallSeconds === null ||
    typeof rawStages !== "object" ||
    rawStages === null
  ) {
    return null;
  }
  const stages = Object.entries(rawStages)
    .flatMap(([id, seconds]) => {
      if (typeof seconds !== "number" || !Number.isFinite(seconds)) return [];
      const share =
        typeof rawShares === "object" && rawShares !== null
          ? Reflect.get(rawShares, id)
          : null;
      return [
        {
          id,
          seconds,
          sharePercent:
            typeof share === "number" && Number.isFinite(share) ? share : null,
        },
      ];
    })
    .sort((left, right) => right.seconds - left.seconds);
  const dominantStage = Reflect.get(value, "dominant_stage");
  return {
    tracksProfiled,
    wallSeconds,
    realtimeFactor: numberProperty(value, "audio_realtime_factor"),
    dominantStage: typeof dominantStage === "string" ? dominantStage : null,
    stages,
  };
}

function formatDuration(seconds: number): string {
  if (seconds < 60) return `${seconds.toFixed(1)} s`;
  const minutes = Math.floor(seconds / 60);
  const remaining = Math.round(seconds - minutes * 60);
  return `${minutes}m ${remaining}s`;
}

function stageLabel(id: string): string {
  const labels: Record<string, string> = {
    probe: "File probe",
    decode_and_frames: "Decode and frame metrics",
    spectrum: "Spectrum (NumPy FFT)",
    feature_summary: "Feature summaries",
    voice: "Voice model",
    ebu_loudness: "EBU loudness",
    finalize: "Final document",
  };
  return labels[id] ?? id.replaceAll("_", " ");
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

function voiceAnalyzerMessage(summary: LibraryContextSummary): string {
  const { model_filename: filename, reason } = summary.voice_analyzer;
  if (summary.voice_analyzer.status === "ready") return "Voice model ready";
  if (summary.voice_analyzer.status === "not_configured") return "Voice model not enabled";
  const reasons: Record<Exclude<typeof reason, null>, string> = {
    model_missing: `${filename} is missing from the configured model mount.`,
    model_unreadable: `${filename} cannot be read by the application.`,
    unsupported_model: `${filename} does not match the supported checksum.`,
    runtime_missing: "The optional Essentia voice runtime is missing from this image.",
  };
  return reason === null ? "Voice model unavailable" : reasons[reason];
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
  const performance = latest === undefined ? null : analysisPerformance(latest);
  const analysisWorkers =
    latest === undefined ? null : resultNumber(latest, "analysis_workers");
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

      {summary !== null ? (
        <div
          className={`assistant-voice-readiness is-${summary.voice_analyzer.status}`}
          role="status"
        >
          <strong>{voiceAnalyzerMessage(summary)}</strong>
          {summary.voice_analyzer.status === "unavailable" ? (
            <span>Fix the deployment model setup before rebuilding context.</span>
          ) : null}
        </div>
      ) : null}

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
          {latest.status === "succeeded" && performance?.tracksProfiled ? (
            <details className="assistant-analysis-performance">
              <summary>Performance profile</summary>
              <p>
                {performance.tracksProfiled} tracks profiled ·{" "}
                {formatDuration(performance.wallSeconds)} wall time
                {performance.realtimeFactor === null
                  ? ""
                  : ` · ${performance.realtimeFactor.toFixed(1)}× real-time`}
                {analysisWorkers === null
                  ? ""
                  : ` · ${analysisWorkers} ${analysisWorkers === 1 ? "worker" : "workers"}`}
              </p>
              {performance.dominantStage !== null ? (
                <p>
                  Largest measured stage: {stageLabel(performance.dominantStage)}
                </p>
              ) : null}
              <dl>
                {performance.stages.map((stage) => (
                  <div key={stage.id}>
                    <dt>{stageLabel(stage.id)}</dt>
                    <dd>
                      {formatDuration(stage.seconds)}
                      {stage.sharePercent === null
                        ? ""
                        : ` · ${stage.sharePercent.toFixed(1)}%`}
                    </dd>
                  </div>
                ))}
              </dl>
            </details>
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
