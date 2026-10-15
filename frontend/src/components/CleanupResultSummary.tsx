import { CleanupCatalogCopy } from "@/components/CleanupCatalogCopy";
import type { CleanupApplySummary } from "@/components/cleanupApply";
import type { BackgroundJob, CleanupEnrichmentResult } from "@/core/api";

/** Counts, skips and the journal reference after an apply. */
export function CleanupDoneSummary({ summary }: { summary: CleanupApplySummary }) {
  return (
    <div className="cleanup-done">
      <p>
        Applied <strong>{summary.applied}</strong> change
        {summary.applied === 1 ? "" : "s"}
        {summary.skipped.length > 0 ? (
          <>
            , skipped <strong>{summary.skipped.length}</strong>
          </>
        ) : null}
        .
      </p>
      {summary.acceptedTags > 0 || summary.tagFailures > 0 ? (
        <p>
          Accepted <strong>{summary.acceptedTags}</strong> database mood tag
          {summary.acceptedTags === 1 ? "" : "s"}
          {summary.tagFailures > 0 ? `; ${summary.tagFailures} could not be reviewed` : ""}.
        </p>
      ) : null}
      {summary.skipped.length > 0 ? (
        <ul className="cleanup-skips">
          {summary.skipped.slice(0, 6).map((s, i) => (
            <li key={`${s.track_id}-${i}`} className="muted small">
              track #{s.track_id}: {s.reason}
            </li>
          ))}
          {summary.skipped.length > 6 ? (
            <li className="muted small">…and {summary.skipped.length - 6} more</li>
          ) : null}
        </ul>
      ) : null}
      {summary.batchId !== null ? (
        <p className="muted small">
          The filename, folder, and embedded-tag changes are journaled as run #{summary.batchId} —
          download it for safekeeping, or revert it later from History.
        </p>
      ) : null}
    </div>
  );
}

/** What a finished catalog lookup found, with export of the original job. */
export function CleanupCatalogSummary({
  job,
  summary,
  onDownload,
}: {
  job: BackgroundJob;
  summary: CleanupEnrichmentResult;
  onDownload: () => void;
}) {
  const incomplete = summary.plans.filter((plan) => plan.partial);
  const incompleteUnmatched = incomplete.filter((plan) => plan.status === "unmatched").length;
  return (
    <>
      <div className="cleanup-catalog-summary" role="status">
        <strong>{summary.identified}</strong> identified
        {summary.fingerprinted > 0
          ? ` · ${summary.fingerprinted} needed fingerprinting`
          : ""}
        {summary.unmatched > 0 ? ` · ${summary.unmatched} unmatched` : ""}
        {summary.failed > 0 ? ` · ${summary.failed} failed` : ""}
        {summary.cached > 0 ? ` · ${summary.cached} reused from cache` : ""}
        {incomplete.length > 0 && (
          <p>
            Catalog evidence is incomplete for {incomplete.length} track{incomplete.length === 1 ? "" : "s"}.
            {incompleteUnmatched > 0
              ? ` ${incompleteUnmatched} of the unmatched tracks had incomplete lookups.`
              : ""}
            {" "}Available suggestions remain reviewable. Run the lookup again to retry missing evidence.
          </p>
        )}
      </div>
      <p>
        <button type="button" className="btn-ghost" onClick={onDownload}>
          Download catalog results
        </button>
        <span className="muted small"> Saves this run's original proposals and evidence as JSON.</span>
      </p>
      <CleanupCatalogCopy key={job.id} job={job} />
    </>
  );
}
