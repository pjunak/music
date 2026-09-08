import type { BackgroundJob } from "@/core/api";
import { modelTaggingResultFromJob } from "./modelTaggingJobs";
import { TaggingYieldSummary } from "./TaggingYieldSummary";
import { taggingYield } from "./taggingYield";

export function TaggingRunOutcome({ job }: { job: BackgroundJob | null }) {
  const result = modelTaggingResultFromJob(job);
  if (!result) return taggingYield(job?.result?.feature_progress)
    ? <TaggingYieldSummary value={job?.result?.feature_progress} partial />
    : <p>See the run status and saved results for completed work.</p>;
  return <div aria-label="Tagging run outcome">
    <strong>{result.stopped_empty_batch ? "Stopped after a request returned no tags" : result.suggested_tags === 0 ? "Analysis finished without tag suggestions" : result.suggested_tags != null ? "Tag suggestions are ready for review" : "Model analysis completed"}</strong>
    {result.processed_tracks != null ? <TaggingYieldSummary value={result} /> : <p>This older run did not record separate counts for suggestions and empty results.</p>}
    <p>{result.updated_profiles} profiles saved; {result.unchanged_profiles} already current{result.deferred_tracks ? `; ${result.deferred_tracks} deferred by the track limit` : ""}{result.skipped_changed_tracks ? `; ${result.skipped_changed_tracks} changed during the run and were not saved` : ""}.</p>
    {result.remaining_tracks ? <p>{result.remaining_tracks} planned tracks were not sent. To continue, start a new run with “Rebuild” off; current results will be skipped. Disable the no-tag stop only if you deliberately want to continue despite empty results.</p> : null}
    {result.suggested_tags === 0 ? <p>Review the saved explanations and evidence before spending on another run.</p> : null}
  </div>;
}
