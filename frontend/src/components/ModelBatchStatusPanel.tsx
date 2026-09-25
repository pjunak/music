import { modelRunReviewUrl } from "@/views/assistant/tagProvenance";
import { useEffect, useRef, useState } from "react";
import { assistantApi, jobsApi, type ModelBatchStatus } from "@/core/api";
import { ModelUsageSummary } from "@/views/assistant/ModelUsageSummary";
import { TaggingYieldSummary } from "@/views/assistant/TaggingYieldSummary";
import { TaggingRunExport } from "@/views/assistant/TaggingRunExport";
import { confirmDialog } from "./confirmDialog";

const TERMINAL = new Set(["completed", "failed", "expired", "cancelled"]);
const ACTIVE_JOB = new Set(["queued", "running", "cancel_requested"]);

export function ModelBatchStatusPanel({ id }: { id: string | null }) {
  const [status, setStatus] = useState<ModelBatchStatus | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [busy, setBusy] = useState(false);
  const [remoteId, setRemoteId] = useState("");
  const [refresh, setRefresh] = useState(0);
  const generation = useRef(0);

  useEffect(() => {
    generation.current += 1;
    setStatus(null);
    setError(null);
    setRemoteId("");
    setBusy(false);
    return () => { generation.current += 1; };
  }, [id]);

  useEffect(() => {
    if (!id) return;
    let disposed = false;
    let timer: number | undefined;
    const poll = async () => {
      let terminal = false;
      try {
        const next = await assistantApi.getModelBatch(id);
        if (!disposed) {
          setStatus(next);
          setError(null);
          terminal = TERMINAL.has(next.state);
        }
      } catch (caught) {
        if (!disposed) setError(caught instanceof Error ? caught.message : "Could not read the batch.");
      }
      if (!disposed && !terminal) timer = window.setTimeout(() => void poll(), 10_000);
    };
    void poll();
    return () => { disposed = true; window.clearTimeout(timer); };
  }, [id, refresh]);

  async function act(cancel: boolean, abandon = false) {
    const startedGeneration = generation.current;
    if (abandon && !await confirmDialog({
      title: "Abandon uncertain submission?",
      body: "First check your OpenAI account for this run ID. Abandoning clears the local block but cannot cancel an unknown remote batch or undo charges. Its input file expires after 7 days. Continue only after resolving the batch in your provider account.",
      confirmLabel: "I checked the account; abandon",
      tone: "primary",
    })) return;
    if (!id || startedGeneration !== generation.current) return;
    setBusy(true);
    setError(null);
    try {
      const job = await assistantApi.updateModelBatch(id, cancel, remoteId || undefined, abandon);
      let latest = job;
      while (ACTIVE_JOB.has(latest.status)) {
        await new Promise((resolve) => window.setTimeout(resolve, 1200));
        if (startedGeneration !== generation.current) return;
        latest = await jobsApi.get(job.id);
      }
      if (startedGeneration !== generation.current) return;
      if (latest.status === "failed") throw new Error(latest.error ?? "Batch update failed.");
      setRefresh((value) => value + 1);
    } catch (caught) {
      if (startedGeneration === generation.current) {
        setError(caught instanceof Error ? caught.message : "Batch update failed.");
      }
    } finally {
      if (startedGeneration === generation.current) setBusy(false);
    }
  }

  if (!id) return null;
  return (
    <section className="surface-card" aria-live="polite">
      <h3>Asynchronous mood tagging</h3>
      <p>{status?.state ?? "Loading batch…"}. The server checks submitted batches every five minutes. You can close this page.</p>
      {status?.remote_batch_id ? <p>Provider batch: <code>{status.remote_batch_id}</code></p> : null}
      {status?.state === "submitting" ? (
        <label className="field">
          <span>Recover an uncertain submission: find the batch with music_run_id {id} in OpenAI, then paste its batch ID.</span>
          <input value={remoteId} onChange={(event) => setRemoteId(event.target.value)} maxLength={128} />
        </label>
      ) : null}
      {status && !TERMINAL.has(status.state) ? (
        <div className="button-row">
          <button className="btn-secondary" disabled={busy} onClick={() => void act(false)}>Check and collect results</button>
          <button className="btn-ghost" disabled={busy} onClick={() => void act(true)}>Cancel batch</button>
          {status.state === "submitting" ? (
            <button className="btn-ghost" disabled={busy} onClick={() => void act(true, true)}>Abandon after checking provider account</button>
          ) : null}
        </div>
      ) : null}
      {status?.result ? (
        <>
          <TaggingYieldSummary value={status.result} />
          <p>Saved {String(status.result.updated_profiles ?? 0)} profiles. Rejected or unavailable tracks: {String(Number(status.result.rejected_tracks ?? 0) + Number(status.result.unavailable_or_changed_tracks ?? 0))}. Suggestions still require review in the Mood Library.</p>
          <a href={modelRunReviewUrl(id)}>View saved results from this batch</a>
          <ModelUsageSummary job={{ result: status.result }} />
        </>
      ) : null}
      {status && TERMINAL.has(status.state) ? <TaggingRunExport id={id} result={status.result} /> : null}
      {error ? <p role="alert">{error}</p> : null}
    </section>
  );
}
