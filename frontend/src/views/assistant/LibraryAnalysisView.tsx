import { lazy, Suspense, useCallback, useEffect, useState } from "react";
import { Link } from "react-router-dom";

import type { BackgroundJob, LibraryContextSummary } from "@/core/api";
import { assistantApi, jobsApi } from "@/core/api";
import { toast } from "@/core/toast";

import {
  analysisStatusLabel,
  formatAnalysisTime,
  isAnalysisJobActive,
} from "./analysisJobs";
import { LibraryAnalyzerPanel } from "./LibraryAnalyzerPanel";
import { ModelTaggingPanel } from "./ModelTaggingPanel";

const LibraryTagEditor = lazy(async () => {
  const module = await import("./LibraryTagEditor");
  return { default: module.LibraryTagEditor };
});

export const LIBRARY_CONTEXT_JOB_KIND = "assistant.library-context-analysis";

export function LibraryAnalysisView() {
  const [history, setHistory] = useState<BackgroundJob[]>([]);
  const [summary, setSummary] = useState<LibraryContextSummary | null>(null);
  const [loading, setLoading] = useState(true);
  const [loadError, setLoadError] = useState<string | null>(null);
  const [busy, setBusy] = useState(false);
  const [refreshKey, setRefreshKey] = useState(0);
  const [tagEditorRefreshKey, setTagEditorRefreshKey] = useState(0);
  const refreshTagSuggestions = useCallback(
    () => setTagEditorRefreshKey((value) => value + 1),
    [],
  );

  useEffect(() => {
    let disposed = false;
    let timer: number | undefined;

    async function poll(initial: boolean) {
      if (initial) setLoading(true);
      try {
        const [nextHistory, nextSummary] = await Promise.all([
          jobsApi.list({ kind: LIBRARY_CONTEXT_JOB_KIND, limit: 10 }),
          assistantApi.getLibraryContextSummary(),
        ]);
        if (disposed) return;
        setHistory(nextHistory);
        setSummary(nextSummary);
        setLoadError(null);
        timer = window.setTimeout(
          () => void poll(false),
          isAnalysisJobActive(nextHistory[0]) ? 1500 : 5000,
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

  function replaceLatest(job: BackgroundJob, refresh = false) {
    setHistory((current) => [job, ...current.filter((item) => item.id !== job.id)]);
    if (refresh) setRefreshKey((value) => value + 1);
  }

  async function start(force: boolean) {
    setBusy(true);
    try {
      const job = await assistantApi.startLibraryContextAnalysis(force);
      replaceLatest(job);
      toast.success(
        force ? "Context rebuild queued" : "Context analysis queued",
        "The multistep analysis continues on the server and checkpoints every track.",
      );
      setRefreshKey((value) => value + 1);
    } catch (error) {
      toast.error(
        "Context analysis could not start",
        error instanceof Error ? error.message : undefined,
      );
    } finally {
      setBusy(false);
    }
  }

  async function cancel() {
    const latest = history[0];
    if (latest === undefined) return;
    setBusy(true);
    try {
      replaceLatest(await jobsApi.cancel(latest.id), true);
    } catch (error) {
      toast.error("Cancellation failed", error instanceof Error ? error.message : undefined);
    } finally {
      setBusy(false);
    }
  }

  async function retry() {
    const latest = history[0];
    if (latest === undefined) return;
    setBusy(true);
    try {
      replaceLatest(await jobsApi.retry(latest.id), true);
    } catch (error) {
      toast.error("Retry failed", error instanceof Error ? error.message : undefined);
    } finally {
      setBusy(false);
    }
  }

  return (
    <div className="assistant-analysis-view">
      <header className="assistant-page-header assistant-analysis-header">
        <div>
          <p className="assistant-eyebrow">One durable local workflow</p>
          <h1>Library context analysis</h1>
          <p>
            Decode each track once into factual, time-aware context for mood tagging:
            dynamics, rhythmic development, spectral movement, tempo, sections, and
            repetition. The analyzer never suggests mood tags itself.
          </p>
        </div>
        <div className="assistant-algorithm-list" aria-label="Analysis contract">
          <span className="assistant-algorithm">local-context/v1</span>
          <span className="assistant-algorithm">checkpointed per track</span>
          <Link to="/assistant/context">Browse track context</Link>
        </div>
      </header>

      {loadError !== null ? (
        <div className="assistant-analysis-error" role="alert">
          <span>{loadError}</span>
          <button type="button" onClick={() => setRefreshKey((value) => value + 1)}>
            Retry
          </button>
        </div>
      ) : null}

      <LibraryAnalyzerPanel
        id="context-analysis"
        title="Comprehensive track context"
        description="Measure the whole track, preserve important changes over time, and condense the result into bounded evidence the tagging model can use."
        analyzer="local-context/v1"
        history={history}
        summary={summary}
        loading={loading}
        actionBusy={busy}
        progressLabel="Library context analysis progress"
        emptyTitle="No track context has been built yet"
        emptyDescription="Start the server-side pass. It can take considerably longer than the old signal scan, but completed tracks are saved immediately and do not need to be repeated."
        analyzeLabel="Build library context"
        checkLabel="Analyze new and changed tracks"
        rebuildTitle="Decode and recompute every track even when its indexed source is unchanged"
        coverageNote="The output is descriptive evidence only. It does not claim a mood, genre, instrument, terrain, or scene and it never writes tags to audio files."
        showFailureStat
        onStart={(force) => void start(force)}
        onCancel={() => void cancel()}
        onRetry={() => void retry()}
      />

      {summary !== null ? (
        <section className="surface-card assistant-context-coverage">
          <div className="assistant-section-heading">
            <div>
              <p className="assistant-eyebrow">Tagging readiness</p>
              <h2>Context coverage</h2>
            </div>
            <Link to="/assistant/context">Inspect individual tracks</Link>
          </div>
          <div className="assistant-model-tagging-stats">
            <div><strong>{summary.full_tracks}</strong><span>Full context</span></div>
            <div><strong>{summary.partial_tracks}</strong><span>Partial context</span></div>
            <div><strong>{summary.missing_tracks}</strong><span>Not analyzed</span></div>
            <div><strong>{summary.failed_tracks + summary.stale_tracks}</strong><span>Failed or stale</span></div>
          </div>
        </section>
      ) : null}

      <ModelTaggingPanel onSuggestionsChanged={refreshTagSuggestions} />

      <Suspense
        fallback={
          <section className="surface-card assistant-tag-workspace">
            <p className="muted">Loading mood-library editor…</p>
          </section>
        }
      >
        <LibraryTagEditor refreshKey={tagEditorRefreshKey} />
      </Suspense>

      <section className="surface-card assistant-analysis-history">
        <div className="assistant-section-heading">
          <div>
            <p className="assistant-eyebrow">Persistent history</p>
            <h2>Recent context jobs</h2>
          </div>
        </div>
        {history.length === 0 ? (
          <p className="muted">Completed and interrupted runs will appear here.</p>
        ) : (
          <div className="assistant-job-history-list">
            {history.map((job) => (
              <div className="assistant-job-history-row" key={job.id}>
                <span className={`assistant-job-status is-${job.status}`}>
                  {analysisStatusLabel(job.status)}
                </span>
                <span>Track context</span>
                <span>{job.progress_phase || "Queued"}</span>
                <span>{formatAnalysisTime(job.created_at)}</span>
                <span>Attempt {job.attempts || 1}</span>
              </div>
            ))}
          </div>
        )}
      </section>
    </div>
  );
}
