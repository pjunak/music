import { lazy, Suspense, useEffect, useMemo, useState } from "react";

import type { BackgroundJob, LibraryAnalysisSummary } from "@/core/api";
import { assistantApi, jobsApi } from "@/core/api";
import { toast } from "@/core/toast";

import {
  analysisStatusLabel,
  formatAnalysisTime,
  isAnalysisJobActive,
} from "./analysisJobs";
import { LibraryAnalyzerPanel } from "./LibraryAnalyzerPanel";

const LibraryTagEditor = lazy(async () => {
  const module = await import("./LibraryTagEditor");
  return { default: module.LibraryTagEditor };
});

const METADATA_JOB_KIND = "assistant.library-analysis";
const AUDIO_JOB_KIND = "assistant.library-audio-analysis";
type AnalyzerKey = "metadata" | "audio";

export function LibraryAnalysisView() {
  const [metadataHistory, setMetadataHistory] = useState<BackgroundJob[]>([]);
  const [audioHistory, setAudioHistory] = useState<BackgroundJob[]>([]);
  const [metadataSummary, setMetadataSummary] =
    useState<LibraryAnalysisSummary | null>(null);
  const [audioSummary, setAudioSummary] = useState<LibraryAnalysisSummary | null>(
    null,
  );
  const [loading, setLoading] = useState(true);
  const [loadError, setLoadError] = useState<string | null>(null);
  const [busyAnalyzer, setBusyAnalyzer] = useState<AnalyzerKey | null>(null);
  const [refreshKey, setRefreshKey] = useState(0);

  useEffect(() => {
    let disposed = false;
    let timer: number | undefined;

    async function poll(initial: boolean) {
      if (initial) setLoading(true);
      try {
        const [metadataJobs, audioJobs, nextMetadataSummary, nextAudioSummary] =
          await Promise.all([
            jobsApi.list({ kind: METADATA_JOB_KIND, limit: 10 }),
            jobsApi.list({ kind: AUDIO_JOB_KIND, limit: 10 }),
            assistantApi.getLibraryAnalysisSummary(),
            assistantApi.getLibraryAudioAnalysisSummary(),
          ]);
        if (disposed) return;
        setMetadataHistory(metadataJobs);
        setAudioHistory(audioJobs);
        setMetadataSummary(nextMetadataSummary);
        setAudioSummary(nextAudioSummary);
        setLoadError(null);
        const hasActiveJob =
          isAnalysisJobActive(metadataJobs[0]) || isAnalysisJobActive(audioJobs[0]);
        timer = window.setTimeout(() => void poll(false), hasActiveJob ? 1500 : 5000);
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

  const combinedHistory = useMemo(
    () =>
      [...metadataHistory, ...audioHistory].sort((left, right) =>
        right.created_at.localeCompare(left.created_at),
      ),
    [audioHistory, metadataHistory],
  );

  function historyFor(analyzer: AnalyzerKey): BackgroundJob[] {
    return analyzer === "metadata" ? metadataHistory : audioHistory;
  }

  function replaceLatest(analyzer: AnalyzerKey, job: BackgroundJob, refresh = false) {
    const setter = analyzer === "metadata" ? setMetadataHistory : setAudioHistory;
    setter((current) => [job, ...current.filter((item) => item.id !== job.id)]);
    if (refresh) setRefreshKey((value) => value + 1);
  }

  async function start(analyzer: AnalyzerKey, force: boolean) {
    setBusyAnalyzer(analyzer);
    try {
      const job =
        analyzer === "metadata"
          ? await assistantApi.startLibraryAnalysis(force)
          : await assistantApi.startLibraryAudioAnalysis(force);
      replaceLatest(analyzer, job);
      const title =
        analyzer === "metadata"
          ? force
            ? "Library rebuild queued"
            : "Library analysis queued"
          : force
            ? "Audio rebuild queued"
            : "Audio analysis queued";
      toast.success(title, "You can leave this page; progress is stored on the server.");
      setRefreshKey((value) => value + 1);
    } catch (error) {
      toast.error(
        "Analysis could not start",
        error instanceof Error ? error.message : undefined,
      );
    } finally {
      setBusyAnalyzer(null);
    }
  }

  async function cancel(analyzer: AnalyzerKey) {
    const latest = historyFor(analyzer)[0];
    if (latest === undefined) return;
    setBusyAnalyzer(analyzer);
    try {
      const job = await jobsApi.cancel(latest.id);
      replaceLatest(analyzer, job, true);
    } catch (error) {
      toast.error(
        "Cancellation failed",
        error instanceof Error ? error.message : undefined,
      );
    } finally {
      setBusyAnalyzer(null);
    }
  }

  async function retry(analyzer: AnalyzerKey) {
    const latest = historyFor(analyzer)[0];
    if (latest === undefined) return;
    setBusyAnalyzer(analyzer);
    try {
      const job = await jobsApi.retry(latest.id);
      replaceLatest(analyzer, job, true);
    } catch (error) {
      toast.error("Retry failed", error instanceof Error ? error.message : undefined);
    } finally {
      setBusyAnalyzer(null);
    }
  }

  return (
    <div className="assistant-analysis-view">
      <header className="assistant-page-header assistant-analysis-header">
        <div>
          <p className="assistant-eyebrow">Durable server-side work</p>
          <h1>Library analysis</h1>
          <p>
            Build reusable local evidence for the whole library. Jobs continue on
            the server and this page restores their progress after refresh or reopen.
          </p>
        </div>
        <div className="assistant-algorithm-list" aria-label="Available analyzers">
          <span className="assistant-algorithm">local-metadata/v1</span>
          <span className="assistant-algorithm">local-audio/v1</span>
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
        id="metadata-analysis"
        title="Metadata profiles"
        description="Turn filenames, paths, genres, origins, and BPM tags into explainable local suggestions."
        analyzer="local-metadata/v1"
        history={metadataHistory}
        summary={metadataSummary}
        loading={loading}
        actionBusy={busyAnalyzer === "metadata"}
        progressLabel="Library analysis progress"
        emptyTitle="No library analysis has run yet"
        emptyDescription="Start the local pass to turn existing metadata and BPM values into reusable, versioned mood profiles."
        analyzeLabel="Analyze library"
        checkLabel="Check for changes"
        rebuildTitle="Recompute every profile even when its source metadata is unchanged"
        coverageNote="This pass uses existing metadata only. Its generated tags remain separate until you explicitly review them below."
        showFailureStat={false}
        onStart={(force) => void start("metadata", force)}
        onCancel={() => void cancel("metadata")}
        onRetry={() => void retry("metadata")}
      />

      <LibraryAnalyzerPanel
        id="audio-analysis"
        title="Audio signal profiles"
        description="Measure level, dynamics, high-frequency content, transient activity, and a tempo estimate when the pulse is stable."
        analyzer="local-audio/v1"
        history={audioHistory}
        summary={audioSummary}
        loading={loading}
        actionBusy={busyAnalyzer === "audio"}
        progressLabel="Audio signal analysis progress"
        emptyTitle="No audio signal analysis has run yet"
        emptyDescription="Start the server-side pass. It processes one track at a time, checkpoints failures, and can continue after a restart."
        analyzeLabel="Analyze audio signals"
        checkLabel="Check audio changes"
        rebuildTitle="Decode and remeasure every track even when the indexed file is unchanged"
        coverageNote="These are measured signal features and conservative proxies. They do not automatically claim a mood, genre, instrument, or D&D scene tag."
        showFailureStat
        onStart={(force) => void start("audio", force)}
        onCancel={() => void cancel("audio")}
        onRetry={() => void retry("audio")}
      />

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
            <h2>Recent analysis jobs</h2>
          </div>
        </div>
        {combinedHistory.length === 0 ? (
          <p className="muted">Completed and interrupted runs will appear here.</p>
        ) : (
          <div className="assistant-job-history-list">
            {combinedHistory.map((job) => (
              <div className="assistant-job-history-row" key={job.id}>
                <span className={`assistant-job-status is-${job.status}`}>
                  {analysisStatusLabel(job.status)}
                </span>
                <span>{job.kind === AUDIO_JOB_KIND ? "Audio signal" : "Metadata"}</span>
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
