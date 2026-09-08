import type { ModelTaggingLimits } from "@/core/api";

export function ModelTaggingRunControls({ limits, onLimits, mode, onMode, batchAvailable }: {
  limits: ModelTaggingLimits; onLimits: (limits: ModelTaggingLimits) => void;
  mode: "standard" | "batch"; onMode: (mode: "standard" | "batch") => void; batchAvailable: boolean;
}) {
  return <section>
    <h3 className="section-label">Run limits</h3>
    <p className="muted">Start with 20 varied tracks. A later run skips current results, including completed analyses that returned no tags.</p>
    <label className="checkbox-row"><input type="checkbox" checked={limits.stop_on_empty_batch !== false} onChange={(event) => onLimits({ ...limits, stop_on_empty_batch: event.target.checked })} /><span>Stop when a request returns no tags</span></label>
    <p className="muted">Completed results are kept. A new run can continue with unprocessed tracks. For asynchronous Batch, this protection limits the pilot to one request; disable it deliberately to submit a larger batch after reviewing the pilot.</p>
    {([
      ["max_tracks", "Maximum tracks", 1, 10000],
      ["max_requests", "Maximum model requests, including corrections", 1, 1000],
      ["max_token_reservation", "Token reservation limit", 1000, 100000000],
    ] as const).map(([key, label, min, max]) => <label className="field" key={key}>
      <span>{label}</span><input type="number" min={min} max={max} value={limits[key]}
        onChange={(event) => onLimits({ ...limits, [key]: Number(event.target.value) })} />
    </label>)}
    <p className="muted">Reservation counts input bytes conservatively plus the output allowance. It is not a price estimate or an account-wide spending limit.</p>
    <label className="field"><span>Processing</span><select value={mode} onChange={(event) => onMode(event.target.value === "batch" ? "batch" : "standard")}>
      <option value="standard">Standard requests</option>
      <option value="batch" disabled={!batchAvailable}>OpenAI Batch — asynchronous, up to 24 hours</option>
    </select></label>
    {mode === "batch" ? <p className="muted">Uploads the disclosed metadata and context as a file. No audio or names of files are uploaded. OpenAI must support Batch for the configured model. There are no automatic corrective calls or resubmissions. The server collects results after completion and attempts to delete its files. Input files expire after 7 days; output files may remain for up to 30 days. Cancellation can still incur charges for completed work.</p> : null}
  </section>;
}
