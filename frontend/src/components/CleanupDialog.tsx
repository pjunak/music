import { useEffect, useMemo, useRef, useState } from "react";

import { applyCleanupSelection } from "@/components/cleanupApply";
import type { CleanupApplySummary } from "@/components/cleanupApply";
import { CleanupConfigureStep } from "@/components/CleanupConfigureStep";
import { CleanupHistoryPanel } from "@/components/CleanupHistoryPanel";
import {
  CleanupApplyingStep,
  CleanupCheckingStep,
  CleanupEnrichingStep,
} from "@/components/CleanupProgressSteps";
import { CleanupRejectedPanel } from "@/components/CleanupRejectedPanel";
import { CleanupCatalogSummary, CleanupDoneSummary } from "@/components/CleanupResultSummary";
import {
  catalogTagKey,
  folderOf,
  mergeEnrichment,
  rejectedOperationIds,
  replaceEdition,
  reviewProposal,
  selectUnambiguous,
} from "@/components/cleanupReview";
import { CleanupReviewStep } from "@/components/CleanupReviewStep";
import { MAX_CATALOG_TRACKS, initialCleanupConfig } from "@/components/cleanupRules";
import type { CleanupConfig } from "@/components/cleanupRules";
import { Modal } from "@/components/Modal";
import { cleanupApi, jobsApi } from "@/core/api";
import type {
  BackgroundJob,
  CleanupAnalyzeResult,
  CleanupCatalogTagSuggestion,
  CleanupEnrichmentResult,
  CleanupModelResult,
  CleanupFolderSuggestion,
  CleanupOp,
  CleanupOpIn,
  CleanupScope,
  CleanupTrackPlan,
  CleanupReviewProposal,
} from "@/core/api";
import { toast } from "@/core/toast";

/** Library cleanup — "find and fix common rip/download residue".
 *
 *  One reusable flow, rendered by the Assistant workspace and retained as a
 *  modal-compatible wrapper: configure (scope + rules) → review
 *  (every proposed change as an old → new diff with its own checkbox;
 *  low-confidence guesses start unticked) → apply (chunked, real progress;
 *  filesystem and embedded-metadata changes are journaled) → done (counts, skips,
 *  journal download). The shared History panel lists past runs with one-click
 *  revert plus revert-from-file for a downloaded journal. Nothing is written
 *  without an explicit Apply on the reviewed diff.
 */

type Step =
  | "configure"
  | "checking"
  | "enriching"
  | "review"
  | "applying"
  | "done"
  | "rejected"
  | "history";

// Names per /verify call — the server paces MusicBrainz at 1 req/s with
// two queries per name, so 5 keeps each request ~10s.
const VERIFY_CHUNK = 5;
const CLEANUP_ENRICHMENT_JOB_KIND = "library.cleanup-enrichment";

function enrichmentResult(job: BackgroundJob): CleanupEnrichmentResult | null {
  const result = job.result;
  if (
    job.kind !== CLEANUP_ENRICHMENT_JOB_KIND ||
    result?.schema !== "library-cleanup-enrichment/v1" ||
    !Array.isArray(result.plans)
  ) {
    return null;
  }
  return result as unknown as CleanupEnrichmentResult;
}

export interface CleanupWorkflowProps {
  /** Currently-browsed folder ("" = music root). */
  path: string;
  /** Ticked track ids from the Library list — offered as a scope. */
  checkedIds: number[];
  onClose: () => void;
  /** Called whenever anything was written (apply or revert) so the host
   *  view refreshes its tree + track list. */
  onApplied: () => void;
  presentation?: "modal" | "workspace";
  /** Workspace mount sends history to its dedicated route. The modal keeps
   *  the same reusable panel inline so older entry points remain complete. */
  onOpenHistory?: () => void;
  startInRejected?: boolean;
}

export function CleanupWorkflow({
  path: initialPath,
  checkedIds: initialCheckedIds,
  onClose,
  onApplied,
  presentation = "workspace",
  onOpenHistory,
  startInRejected = false,
}: CleanupWorkflowProps) {
  const [path, setPath] = useState(initialPath);
  const [checkedIds, setCheckedIds] = useState(initialCheckedIds);
  const [step, setStep] = useState<Step>(startInRejected ? "rejected" : "configure");
  const [rejectedIds, setRejectedIds] = useState<Set<string>>(new Set());
  const [restoredReview, setRestoredReview] = useState(false);
  const [config, setConfig] = useState<CleanupConfig>(() =>
    initialCleanupConfig(checkedIds.length > 0 ? "tracks" : path ? "folder" : "all"),
  );
  const [editions, setEditions] = useState<Record<string, string>>({});
  const [busy, setBusy] = useState(false);
  const [result, setResult] = useState<CleanupAnalyzeResult | null>(null);
  const [ticked, setTicked] = useState<Set<string>>(new Set());
  const [catalogTags, setCatalogTags] = useState<CleanupCatalogTagSuggestion[]>([]);
  const [tickedCatalogTags, setTickedCatalogTags] = useState<Set<string>>(new Set());
  const [enrichmentJob, setEnrichmentJob] = useState<BackgroundJob | null>(null);
  const [enrichmentSummary, setEnrichmentSummary] = useState<CleanupEnrichmentResult | null>(null);
  const [catalogScopeNotice, setCatalogScopeNotice] = useState<string | null>(null);
  const [progress, setProgress] = useState({ done: 0, total: 0 });
  const [checkProgress, setCheckProgress] = useState({ done: 0, total: 0 });
  // Set by the Skip button (or closing the dialog) to stop further lookup
  // chunks; whatever resolved so far still feeds the re-analysis.
  const skipCheckRef = useRef(false);
  const mountedRef = useRef(true);
  const [summary, setSummary] = useState<CleanupApplySummary | null>(null);

  useEffect(
    () => () => {
      mountedRef.current = false;
    },
    [],
  );

  function updateConfig(patch: Partial<CleanupConfig>) {
    setConfig((current) => ({ ...current, ...patch }));
  }

  const { scopeType, recursive, rules, useCatalogs, refreshCatalogs, imports } = config;
  const scope: CleanupScope =
    scopeType === "tracks"
      ? { type: "tracks", track_ids: checkedIds }
      : scopeType === "folder"
        ? { type: "folder", path, recursive }
        : { type: "all" };

  const scopeLabel = restoredReview ? "restored rejected suggestion" :
    scopeType === "tracks"
      ? `${checkedIds.length} selected track${checkedIds.length === 1 ? "" : "s"}`
      : scopeType === "folder"
        ? `folder “${path || "(root)"}”${recursive ? "" : " (no subfolders)"}`
        : "entire library";

  const plans: CleanupTrackPlan[] = useMemo(() => (result?.plans ?? []).map((plan) => ({ ...plan, ops: plan.ops.filter((op) => !rejectedIds.has(op.op_id)) })), [result, rejectedIds]);
  const folderSuggestions: CleanupFolderSuggestion[] = useMemo(
    () => (result?.folders ?? []).filter((folder) => !rejectedIds.has(folder.op_id)),
    [result, rejectedIds],
  );

  async function prepareReview(
    r: CleanupAnalyzeResult,
    tags: CleanupCatalogTagSuggestion[] = [],
  ): Promise<boolean> {
    if (r.plans.length === 0 && r.folders.length === 0 && tags.length === 0) {
      toast.success(
        "Nothing to clean",
        `Scanned ${r.scanned} track${r.scanned === 1 ? "" : "s"} — no issues matched the enabled rules.`,
      );
      return false;
    }
    const rejected = await rejectedOperationIds(r);
    setRejectedIds(rejected);
    setResult(r);
    setCatalogTags(tags);
    setTickedCatalogTags(new Set());
    // High-confidence suggestions start ticked; guesses (including folder
    // rebuilds) start unticked so a quick "Apply" only commits the safe set.
    setTicked(
      selectUnambiguous([
        ...r.plans.flatMap((p) =>
          p.ops.filter((o) => o.confidence === "high" && !rejected.has(o.op_id)).map((o) => o.op_id),
        ),
        ...r.folders.filter((f) => f.confidence === "high" && !rejected.has(f.op_id)).map((f) => f.op_id),
      ], r.plans.flatMap((p) => p.ops)),
    );
    setStep("review");
    return true;
  }

  /** Resolve unfamiliar names against MusicBrainz. Verdicts are cached
   *  server-side forever, so each distinct name across the whole library
   *  is only ever looked up once — later runs reuse them instantly. */
  async function checkNamesOnline(names: string[]) {
    skipCheckRef.current = false;
    setCheckProgress({ done: 0, total: names.length });
    setStep("checking");
    let failed = 0;
    for (let i = 0; i < names.length; i += VERIFY_CHUNK) {
      if (skipCheckRef.current) return;
      const chunk = names.slice(i, i + VERIFY_CHUNK);
      try {
        const v = await cleanupApi.verify(chunk);
        failed += v.failed.length;
      } catch {
        toast.warn(
          "Online name check unavailable",
          "Continuing with local clues only — unresolved names retry next run.",
        );
        return;
      }
      setCheckProgress({
        done: Math.min(i + VERIFY_CHUNK, names.length),
        total: names.length,
      });
    }
    if (failed > 0) {
      toast.warn(
        `${failed} name lookup${failed === 1 ? "" : "s"} failed`,
        "Those names keep their offline grading and retry on the next run.",
      );
    }
  }

  async function runAnalyze() {
    setRestoredReview(false);
    setBusy(true);
    setEnrichmentJob(null);
    setEnrichmentSummary(null);
    setCatalogScopeNotice(null);
    setEditions({});
    try {
      let r = await cleanupApi.analyze(scope, [...rules]);
      if (r.pending_lookups.length > 0) {
        // One more clue source: settle unknown artist-vs-album names
        // online, then re-analyze with the verdicts folded in.
        await checkNamesOnline(r.pending_lookups);
        r = await cleanupApi.analyze(scope, [...rules]);
      }
      if (useCatalogs && r.scanned > MAX_CATALOG_TRACKS) {
        setCatalogScopeNotice(
          `Catalog lookup was skipped for this ${r.scanned}-track scan. Choose a folder or selection of up to ${MAX_CATALOG_TRACKS} tracks to identify them. Local cleanup suggestions are still available.`,
        );
      } else if (useCatalogs) {
        try {
          setStep("enriching");
          let job = await cleanupApi.enrich(scope, refreshCatalogs, imports);
          if (mountedRef.current) setEnrichmentJob(job);
          while (["queued", "running", "cancel_requested"].includes(job.status)) {
            await new Promise((resolve) => window.setTimeout(resolve, 1_000));
            if (!mountedRef.current) return;
            job = await jobsApi.get(job.id);
            setEnrichmentJob(job);
          }
          if (job.status === "succeeded") {
            const enrichment = enrichmentResult(job);
            if (enrichment !== null) {
              setEnrichmentSummary(enrichment);
              const tags = enrichment.plans.flatMap((plan) => plan.tag_suggestions);
              r = mergeEnrichment(r, enrichment);
              if (!await prepareReview(r, tags)) setStep("configure");
              return;
            }
          }
          toast.warn(
            "Catalog enrichment did not finish",
            job.error ?? "Continuing with local cleanup suggestions only.",
          );
        } catch (error) {
          toast.warn(
            "Catalog enrichment unavailable",
            error instanceof Error
              ? `${error.message} Local cleanup suggestions are still available.`
              : "Continuing with local cleanup suggestions only.",
          );
        }
      }
      setEnrichmentSummary(null);
      if (!await prepareReview(r)) setStep("configure");
    } catch (e) {
      toast.error("Analysis failed", e instanceof Error ? e.message : undefined);
      setStep("configure");
    } finally {
      setBusy(false);
    }
  }

  function toggleCatalogTag(suggestion: CleanupCatalogTagSuggestion) {
    const key = catalogTagKey(suggestion);
    setTickedCatalogTags((current) => {
      const next = new Set(current);
      if (next.has(key)) next.delete(key);
      else next.add(key);
      return next;
    });
  }

  async function updateReview(next: CleanupAnalyzeResult, removed: Set<string>) {
    setBusy(true);
    try {
      const rejected = await rejectedOperationIds(next);
      setRejectedIds(rejected); setResult(next);
      setTicked((current) => new Set([...current].filter((id) => !removed.has(id) && !rejected.has(id))));
      return true;
    } catch (cause) { toast.error("Could not check rejected suggestions", cause instanceof Error ? cause.message : undefined); return false; }
    finally { setBusy(false); }
  }

  async function rejectOperation(op: CleanupOp | CleanupFolderSuggestion, opPath: string) {
    setBusy(true);
    try {
      await cleanupApi.reject(reviewProposal(op, opPath));
      setRejectedIds((current) => new Set([...current, op.op_id]));
      setTicked((current) => new Set([...current].filter((id) => id !== op.op_id)));
      toast.success("Suggestion rejected", "You can find it in Rejected suggestions and restore it later.");
    } catch (cause) { toast.error("Could not reject suggestion", cause instanceof Error ? cause.message : undefined); }
    finally { setBusy(false); }
  }

  function restoreProposal(proposal: CleanupReviewProposal) {
    setCatalogScopeNotice(null);
    const { evidence, evidence_context, ...fields } = proposal;
    const op = { ...fields, ...(evidence ? { evidence } : {}), ...(evidence_context ? { review_context: evidence_context } : {}) };
    setResult({ scanned: 1, pending_lookups: [],
      plans: proposal.kind === "folder_rename" ? [] : [{ track_id: proposal.track_id, path: proposal.path, ops: [{ ...op, kind: proposal.kind }], notes: ["Restored from rejected suggestions. Review and tick this change to apply it."] }],
      folders: proposal.kind === "folder_rename" ? [{ op_id: proposal.op_id, path: proposal.path, old: String(proposal.old), new: String(proposal.new), rules: proposal.rules, confidence: proposal.confidence }] : [],
    });
    setRejectedIds(new Set()); setTicked(new Set()); setCatalogTags([]); setTickedCatalogTags(new Set());
    setEnrichmentSummary(null); setEnrichmentJob(null); setEditions({}); setRestoredReview(true); setStep("review");
  }

  function recheckProposal(proposal: CleanupReviewProposal) {
    setRestoredReview(false);
    setPath(proposal.kind === "folder_rename" ? proposal.path : folderOf(proposal.path));
    setCheckedIds(proposal.track_id > 0 ? [proposal.track_id] : []);
    updateConfig({ scopeType: proposal.track_id > 0 ? "tracks" : "folder", imports: [] });
    setStep("configure");
  }

  async function acceptModelReview(review: CleanupModelResult) {
    if (!result) return;
    const previous = new Set(plans.filter((p) => p.track_id === review.track_id).flatMap((p) => p.ops)
      .filter((op) => op.rules.includes("model_catalog_choice")).map((op) => op.op_id));
    await updateReview({ ...result, plans: result.plans.map((plan) => plan.track_id !== review.track_id ? plan : {
      ...plan, ops: [...plan.ops.filter((op) => !op.rules.includes("model_catalog_choice")), ...review.ops.map((op) => ({ ...op, review_context: JSON.stringify([review.source_signature, review.role_fingerprint]) }))],
      notes: [...plan.notes, `AI review (${review.decision.decision.replaceAll("_", " ")}): ${review.decision.reason}`],
    }) }, previous);
  }

  async function chooseEdition(folder: string, releaseId: string) {
    if (!result) return;
    const replacement = replaceEdition(result, enrichmentSummary?.plans ?? [], folder, releaseId);
    const updated = await updateReview(replacement.result, replacement.removed);
    if (updated) setEditions((current) => ({ ...current, [folder]: releaseId }));
  }

  async function runApply() {
    const trackOps: CleanupOpIn[] = plans.flatMap((p) =>
      p.ops
        .filter((o) => ticked.has(o.op_id))
        .map((o) => ({
          track_id: o.track_id,
          kind: o.kind,
          field: o.field,
          old: o.old,
          new: o.new,
        })),
    );
    // Folder renames go last and deepest-first: a child folder must move
    // before its parent's path shifts under it (the server re-asserts this
    // per chunk, but ordering them here keeps a nested pair from splitting
    // parent-before-child across a chunk boundary).
    const folderOps: CleanupOpIn[] = folderSuggestions
      .filter((f) => ticked.has(f.op_id))
      .slice()
      .sort((a, b) => b.path.split("/").length - a.path.split("/").length)
      .map((f) => ({
        track_id: 0,
        kind: "folder_rename" as const,
        field: null,
        old: f.old,
        new: f.new,
        path: f.path,
      }));
    const ops = [...trackOps, ...folderOps];
    const tagTargets = catalogTags
      .filter((suggestion) => tickedCatalogTags.has(catalogTagKey(suggestion)))
      .map(({ track_id, tag, analyzer_id, source_signature }) => ({
        track_id,
        tag,
        analyzer_id,
        source_signature,
      }));
    if (ops.length === 0 && tagTargets.length === 0) return;
    setStep("applying");
    setProgress({ done: 0, total: ops.length + tagTargets.length });
    const outcome = await applyCleanupSelection(ops, tagTargets, scopeLabel, (done, total) =>
      setProgress({ done, total }),
    );
    if (outcome.tagError !== null) toast.error("Mood tag review stopped partway", outcome.tagError);
    if (outcome.applyError !== null) {
      toast.error(
        "Apply stopped partway",
        `${outcome.applied} change${outcome.applied === 1 ? "" : "s"} landed before the error: ${outcome.applyError}`,
      );
    }
    setSummary(outcome);
    setStep("done");
    if (outcome.applied > 0 || outcome.acceptedTags > 0) onApplied();
  }

  function downloadCatalogResult() {
    if (enrichmentJob?.status !== "succeeded" || enrichmentSummary === null) return;
    let url: string | undefined;
    try {
      // Preserve the original job, including scope and evidence. Review choices
      // and merged local suggestions are not a retained catalog run.
      const blob = new Blob([JSON.stringify(enrichmentJob, null, 2)], { type: "application/json" });
      url = URL.createObjectURL(blob);
      const link = document.createElement("a");
      link.href = url;
      link.download = `cleanup-catalog-${enrichmentJob.id.replace(/[^a-zA-Z0-9_-]/g, "_")}.json`;
      link.click();
    } catch (error) {
      toast.error("Could not download catalog results", error instanceof Error ? error.message : undefined);
    } finally {
      if (url !== undefined) URL.revokeObjectURL(url);
    }
  }

  function prepareEditionLookup(folder: string, releaseId: string) {
    const ids = enrichmentSummary?.plans.filter((plan) => folderOf(plan.path) === folder).map((plan) => plan.track_id) ?? [];
    if (ids.length === 0) return;
    setCheckedIds(ids);
    setPath(folder);
    // A new edition must not inherit another source's field mapping or proposal opt-in.
    updateConfig({
      imports: ids.map((track_id) => ({ track_id, fields: { release_mbid: releaseId } })),
      scopeType: "tracks",
      useCatalogs: true,
      refreshCatalogs: true,
    });
    setStep("configure");
  }

  async function downloadJournal(batchId: number) {
    try {
      const detail = await cleanupApi.batch(batchId);
      const blob = new Blob([JSON.stringify(detail, null, 2)], {
        type: "application/json",
      });
      const a = document.createElement("a");
      a.href = URL.createObjectURL(blob);
      a.download = `cleanup-batch-${batchId}.json`;
      a.click();
      URL.revokeObjectURL(a.href);
    } catch (e) {
      toast.error("Download failed", e instanceof Error ? e.message : undefined);
    }
  }

  // --- footers ---------------------------------------------------------------

  const tickedCount = ticked.size + tickedCatalogTags.size;
  const footer =
    step === "configure" ? (
      <>
        <button type="button" className="btn-ghost" disabled={busy} onClick={() => setStep("rejected")}>Rejected suggestions</button>
        <button
          type="button"
          className="btn-ghost"
          onClick={() => (onOpenHistory ? onOpenHistory() : setStep("history"))}
        >
          History
        </button>
        <button type="button" onClick={onClose}>
          Cancel
        </button>
        <button
          type="button"
          className="btn-primary"
          disabled={busy || rules.size === 0 || (scopeType === "tracks" && checkedIds.length === 0)}
          onClick={() => void runAnalyze()}
        >
          {busy ? "Scanning…" : "Find issues"}
        </button>
      </>
    ) : step === "review" ? (
      <>
        <button type="button" className="btn-ghost" onClick={() => setStep("configure")}>
          Back
        </button>
        <button type="button" onClick={onClose}>
          Cancel
        </button>
        <button
          type="button"
          className="btn-primary"
          disabled={busy || tickedCount === 0}
          onClick={() => void runApply()}
        >
          Apply {tickedCount} change{tickedCount === 1 ? "" : "s"}
        </button>
      </>
    ) : step === "done" ? (
      <>
        <button type="button" className="btn-ghost" onClick={() => setStep("rejected")}>
          Rejected suggestions
        </button>
        {summary?.batchId != null ? (
          <button
            type="button"
            className="btn-secondary"
            onClick={() => void downloadJournal(summary.batchId as number)}
          >
            Download journal
          </button>
        ) : null}
        <button type="button" className="btn-primary" onClick={onClose}>
          Close
        </button>
      </>
    ) : step === "history" || step === "rejected" ? (
      <button type="button" className="btn-ghost" onClick={() => setStep("configure")}>
        Back
      </button>
    ) : step === "checking" ? (
      <button
        type="button"
        className="btn-ghost"
        onClick={() => {
          skipCheckRef.current = true;
        }}
      >
        Skip — use local clues only
      </button>
    ) : step === "enriching" ? (
      <button
        type="button"
        className="btn-ghost"
        disabled={
          enrichmentJob === null ||
          !["queued", "running", "cancel_requested"].includes(enrichmentJob.status)
        }
        onClick={() => {
          if (enrichmentJob !== null) {
            void jobsApi.cancel(enrichmentJob.id).then(setEnrichmentJob).catch(() => undefined);
          }
        }}
      >
        Cancel catalog lookup
      </button>
    ) : undefined; // applying: no actions — let it finish

  const titles: Record<Step, string> = {
    configure: "Clean up library",
    checking: "Checking names online",
    enriching: "Identifying and enriching tracks",
    review: `Review proposed changes — ${scopeLabel}`,
    applying: "Applying changes",
    done: "Cleanup applied",
    history: "Cleanup history",
    rejected: "Rejected suggestions",
  };

  function closeDialog() {
    // Closing mid-check just stops further lookups; resolved verdicts are
    // already cached and benefit the next run.
    skipCheckRef.current = true;
    onClose();
  }

  function stepBody() {
    switch (step) {
      case "configure":
        return (
          <CleanupConfigureStep
            config={config}
            onChange={updateConfig}
            path={path}
            selectedCount={checkedIds.length}
          />
        );
      case "checking":
        return <CleanupCheckingStep progress={checkProgress} />;
      case "enriching":
        return <CleanupEnrichingStep job={enrichmentJob} />;
      case "review":
        return (
          <CleanupReviewStep
            busy={busy}
            scanned={result?.scanned ?? null}
            plans={plans}
            folderSuggestions={folderSuggestions}
            rejectedCount={rejectedIds.size}
            ticked={ticked}
            setTicked={setTicked}
            catalogTags={catalogTags}
            tickedCatalogTags={tickedCatalogTags}
            onToggleCatalogTag={toggleCatalogTag}
            enrichmentJob={enrichmentJob}
            enrichmentSummary={enrichmentSummary}
            editions={editions}
            onShowRejected={() => setStep("rejected")}
            onReject={(op, opPath) => void rejectOperation(op, opPath)}
            onTargetEdition={prepareEditionLookup}
            onChooseEdition={(folder, releaseId) => void chooseEdition(folder, releaseId)}
            onModelReview={(review) => void acceptModelReview(review)}
          />
        );
      case "applying":
        return <CleanupApplyingStep progress={progress} />;
      case "done":
        return summary ? <CleanupDoneSummary summary={summary} /> : null;
      case "rejected":
        return <CleanupRejectedPanel onRestore={restoreProposal} onRecheck={recheckProposal} />;
      case "history":
        return <CleanupHistoryPanel onApplied={onApplied} />;
    }
  }

  const body = <>
    {catalogScopeNotice && !busy && (step === "configure" || step === "review") && (
      <p className="cleanup-catalog-summary" role="status">{catalogScopeNotice}</p>
    )}
    {enrichmentJob?.status === "succeeded" && enrichmentSummary !== null && !busy &&
      (step === "configure" || step === "review" || step === "done") && (
      <CleanupCatalogSummary
        job={enrichmentJob}
        summary={enrichmentSummary}
        onDownload={downloadCatalogResult}
      />
    )}
    {stepBody()}
  </>;

  if (presentation === "modal") {
    return (
      <Modal
        title={titles[step]}
        ariaLabel="Library cleanup"
        className="modal-cleanup"
        onClose={step === "applying" ? () => undefined : closeDialog}
        footer={footer}
        closeButton={step !== "applying"}
      >
        {body}
      </Modal>
    );
  }

  return (
    <section className="cleanup-workspace" aria-labelledby="cleanup-workspace-title">
      <header className="cleanup-workspace-heading">
        <div>
          <p className="assistant-eyebrow">Review-first repair</p>
          <h2 id="cleanup-workspace-title">{titles[step]}</h2>
        </div>
        {step !== "applying" ? (
          <button type="button" className="btn-ghost" onClick={closeDialog}>
            Return to Library
          </button>
        ) : null}
      </header>
      <div className="cleanup-workspace-body">{body}</div>
      {footer !== undefined ? <footer className="cleanup-workspace-footer">{footer}</footer> : null}
    </section>
  );
}

export function CleanupDialog(props: Omit<CleanupWorkflowProps, "presentation">) {
  return <CleanupWorkflow {...props} presentation="modal" />;
}
