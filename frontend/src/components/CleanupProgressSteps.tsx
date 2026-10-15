import type { BackgroundJob } from "@/core/api";

function ProgressBar({
  label,
  ariaLabel,
  value,
  max,
}: {
  label: string;
  ariaLabel: string;
  value: number;
  max: number;
}) {
  return (
    <div className="upload-progress cleanup-progress">
      <div className="upload-progress-label">{label}</div>
      <progress aria-label={ariaLabel} value={value} max={max} />
    </div>
  );
}

function fraction({ done, total }: { done: number; total: number }): number {
  return total > 0 ? done / total : 0;
}

export function CleanupCheckingStep({ progress }: { progress: { done: number; total: number } }) {
  return (
    <div className="cleanup-checking">
      <ProgressBar
        label={`Checking names online — ${progress.done} / ${progress.total}`}
        ariaLabel="Name check progress"
        value={fraction(progress)}
        max={1}
      />
      <p className="muted small">
        Unfamiliar artist/album guesses are checked against MusicBrainz (one
        request per second, as their API asks). Each name is looked up once and
        the verdict is remembered — future cleanups reuse it instantly.
      </p>
    </div>
  );
}

export function CleanupEnrichingStep({ job }: { job: BackgroundJob | null }) {
  return (
    <div className="cleanup-checking cleanup-enriching">
      <ProgressBar
        label={job?.progress_message || "Preparing catalog lookup…"}
        ariaLabel="Catalog enrichment progress"
        value={job?.progress_current ?? 0}
        max={Math.max(job?.progress_total ?? 1, 1)}
      />
      <p className="muted small">
        MusicBrainz searches are paced to one request per second. AcoustID fingerprints are created
        locally and used only as a fallback. You may leave this screen; the durable job keeps its
        progress and cached track results.
      </p>
    </div>
  );
}

export function CleanupApplyingStep({ progress }: { progress: { done: number; total: number } }) {
  return (
    <ProgressBar
      label={`Applying — ${progress.done} / ${progress.total} changes`}
      ariaLabel="Cleanup progress"
      value={fraction(progress)}
      max={1}
    />
  );
}
