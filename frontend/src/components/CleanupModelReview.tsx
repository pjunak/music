import { useEffect, useRef, useState } from "react";
import { cleanupApi, jobsApi } from "@/core/api";
import type { CleanupModelResult, CleanupModelStatus } from "@/core/api";
import { toast } from "@/core/toast";

export function CleanupModelReview({ trackId, catalogJobId, onResult }: {
  trackId: number; catalogJobId: string; onResult: (result: CleanupModelResult) => void;
}) {
  const mounted = useRef(true);
  useEffect(() => { mounted.current = true; return () => { mounted.current = false; }; }, []);
  const [status, setStatus] = useState<CleanupModelStatus | null>(null);
  const [consent, setConsent] = useState(false);
  const [busy, setBusy] = useState(false);
  return <div className="cleanup-evidence">
    {!status && <button type="button" className="btn-link" disabled={busy} onClick={async () => {
      setBusy(true);
      try { setStatus(await cleanupApi.modelStatus()); }
      catch (error) { toast.error("AI setup unavailable", error instanceof Error ? error.message : undefined); }
      finally { setBusy(false); }
    }}>Review ambiguous candidates with AI</button>}
    {status && !status.available && <p>Library cleanup AI needs configuration and passing quality checks in <a href="/assistant/ai">AI setup</a>. ({status.reason_code})</p>}
    {status?.available && <>
      <p>{status.model_id} can suggest one supplied candidate or abstain. Its result starts unchecked.</p>
      <p>Shared: {status.shared_with_provider.join("; ")}. Excluded: {status.never_shared.join("; ")}.</p>
      <label><input type="checkbox" checked={consent} onChange={(event) => setConsent(event.target.checked)} disabled={busy} />Allow one request using this evidence. Provider charges may apply.</label>
      <button type="button" disabled={!consent || busy} onClick={async () => {
        setBusy(true);
        try {
          let job = await cleanupApi.reviewCandidates(trackId, catalogJobId, status.disclosure_version);
          while (mounted.current && ["queued", "running", "cancel_requested"].includes(job.status)) {
            await new Promise((resolve) => window.setTimeout(resolve, 1000));
            if (!mounted.current) return;
            job = await jobsApi.get(job.id);
          }
          if (!mounted.current) return;
          if (job.status !== "succeeded" || job.result?.schema_version !== "assistant-library-cleanup-result/v1") throw new Error(job.error ?? "The candidate review did not finish.");
          onResult(job.result as unknown as CleanupModelResult);
        } catch (error) { toast.error("AI candidate review failed", error instanceof Error ? error.message : undefined); }
        finally { setBusy(false); setConsent(false); }
      }}>{busy ? "Reviewing candidates…" : "Ask AI to review"}</button>
    </>}
  </div>;
}
