import { useEffect, useMemo, useRef, useState } from "react";

import { catalogReviewContext, mergeEnrichment, rejectedOperationIds, reviewProposal, selectUnambiguous, toggleReviewOperation } from "@/components/cleanupReview";
import { CleanupRejectedPanel } from "@/components/CleanupRejectedPanel";
import { CleanupEvidence, CleanupEvidenceImport, CleanupEditionTarget } from "@/components/CleanupEvidence";
import { CleanupModelReview } from "@/components/CleanupModelReview";
import { CleanupHistoryPanel } from "@/components/CleanupHistoryPanel";
import { CleanupCatalogCopy } from "@/components/CleanupCatalogCopy";
import { WarnIcon } from "@/components/icons";
import { Modal } from "@/components/Modal";
import { assistantApi, cleanupApi, jobsApi } from "@/core/api";
import type {
  BackgroundJob,
  CleanupAnalyzeResult,
  CleanupCatalogTagSuggestion,
  CleanupEnrichmentResult,
  CleanupImportedEvidence,
  CleanupModelResult,
  CleanupFolderSuggestion,
  CleanupOp,
  CleanupOpIn,
  CleanupRuleId,
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

type ScopeType = "all" | "folder" | "tracks";
type Step =
  | "configure"
  | "checking"
  | "enriching"
  | "review"
  | "applying"
  | "done"
  | "rejected"
  | "history";

interface RuleMeta {
  id: CleanupRuleId;
  label: string;
  hint: string;
  defaultOn: boolean;
}

const RULE_GROUPS: { label: string; rules: RuleMeta[] }[] = [
  {
    label: "Filename fixes",
    rules: [
      {
        id: "strip_track_numbers",
        label: "Strip leading track numbers",
        hint: "“01 - Title” → “Title” (the number can still go to the tag below)",
        defaultOn: true,
      },
      {
        id: "strip_artist",
        label: "Strip artist from filename",
        hint: "“Artist - Title” → “Title” when the artist is tagged, matches the folder, or is shared by the whole folder",
        defaultOn: true,
      },
      {
        id: "strip_album",
        label: "Strip album from filename",
        hint: "Drops a segment matching the album tag or a folder-wide album prefix",
        defaultOn: true,
      },
      {
        id: "strip_junk",
        label: "Remove junk phrases",
        hint: "“(Official Audio)”, “[320kbps]”, site names, “- YouTube”…",
        defaultOn: true,
      },
      {
        id: "normalize_separators",
        label: "Fix separators",
        hint: "Underscores → spaces, %20, doubled spaces",
        defaultOn: true,
      },
      {
        id: "normalize_case",
        label: "Fix ALL-CAPS / all-lowercase names",
        hint: "“MY SONG” → “My Song” — opinionated, so off by default",
        defaultOn: false,
      },
    ],
  },
  {
    label: "Tag fixes (derived from filename + folder)",
    rules: [
      {
        id: "tag_title",
        label: "Set / clean the title tag",
        hint: "Cleans the same residue out of the title tag, or fills it from the cleaned filename",
        defaultOn: true,
      },
      {
        id: "tag_artist",
        label: "Fill empty artist tag",
        hint: "From an “Artist - …” filename prefix",
        defaultOn: true,
      },
      {
        id: "tag_album",
        label: "Fill empty album tag",
        hint: "From an album segment in the filename",
        defaultOn: true,
      },
      {
        id: "tag_number",
        label: "Fill empty track / disc number",
        hint: "From the stripped leading number or a CD1/CD2 subfolder — pre-ticked only when the folder forms a numbered sequence (01, 02, …)",
        defaultOn: true,
      },
      {
        id: "tag_year",
        label: "Fill empty year tag",
        hint: "From a year marker in the folder name — “Album (2013)”, “2019 - Album”",
        defaultOn: true,
      },
    ],
  },
  {
    label: "Folder names",
    rules: [
      {
        id: "rename_folders",
        label: "Rename messy folders",
        hint: "Tidy folder names (underscores, junk), canonicalize disc/part folders to “Disc 1” / “Part 1”, and — when a name is unusable (“1”, artist + junk) — rebuild it from the tracks’ tags (a guess, starts unticked). Case follows the rule above.",
        defaultOn: true,
      },
    ],
  },
];

const DEFAULT_RULES: CleanupRuleId[] = RULE_GROUPS.flatMap((g) =>
  g.rules.filter((r) => r.defaultOn).map((r) => r.id),
);

const APPLY_CHUNK = 20;
// Names per /verify call — the server paces MusicBrainz at 1 req/s with
// two queries per name, so 5 keeps each request ~10s.
const VERIFY_CHUNK = 5;
const REVIEW_CHUNK = 500;
const CLEANUP_ENRICHMENT_JOB_KIND = "library.cleanup-enrichment";
// Mirrors the provider job's bound; its execution-time check remains authoritative.
const MAX_CATALOG_TRACKS = 500;

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

function opLabel(op: CleanupOp): string {
  if (op.kind === "rename") return "File";
  switch (op.field) {
    case "title":
      return "Title";
    case "artist":
      return "Artist";
    case "album_artist":
      return "Album artist";
    case "album":
      return "Album";
    case "track_no":
      return "Track #";
    case "disc_no":
      return "Disc #";
    case "year":
      return "Year";
    default:
      return op.field ?? "Tag";
  }
}

/** Stable display order for the by-field tick chips. */
const LABEL_ORDER = [
  "Folder",
  "File",
  "Title",
  "Artist",
  "Album artist",
  "Album",
  "Track #",
  "Disc #",
  "Year",
];

function Value({ value }: { value: string | number | null }) {
  if (value === null || value === "") {
    return <em className="muted">(empty)</em>;
  }
  return <>{String(value)}</>;
}

function folderOf(path: string): string {
  const i = path.lastIndexOf("/");
  return i === -1 ? "" : path.slice(0, i);
}

function basename(path: string): string {
  return path.split("/").pop() || path;
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
  const [scopeType, setScopeType] = useState<ScopeType>(
    checkedIds.length > 0 ? "tracks" : path ? "folder" : "all",
  );
  const [recursive, setRecursive] = useState(true);
  const [rules, setRules] = useState<Set<CleanupRuleId>>(new Set(DEFAULT_RULES));
  const [useCatalogs, setUseCatalogs] = useState(true);
  const [refreshCatalogs, setRefreshCatalogs] = useState(false);
  const [imports, setImports] = useState<CleanupImportedEvidence[]>([]);
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
  const [summary, setSummary] = useState<{
    applied: number;
    skipped: { track_id: number; reason: string }[];
    batchId: number | null;
    acceptedTags: number;
    tagFailures: number;
  } | null>(null);

  useEffect(
    () => () => {
      mountedRef.current = false;
    },
    [],
  );

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
  const folderSuggByPath = useMemo(() => {
    const m = new Map<string, CleanupFolderSuggestion>();
    for (const f of folderSuggestions) m.set(f.path, f);
    return m;
  }, [folderSuggestions]);
  const catalogPlanByTrack = useMemo(
    () => new Map(enrichmentSummary?.plans.map((plan) => [plan.track_id, plan]) ?? []),
    [enrichmentSummary],
  );
  const incompleteCatalogPlans = enrichmentSummary?.plans.filter((plan) => plan.partial) ?? [];
  const incompleteUnmatchedCount = incompleteCatalogPlans.filter((plan) => plan.status === "unmatched").length;

  // Every tickable change (track ops + folder renames) flattened to a common
  // {op_id, confidence, label} shape — drives the count, the All/Confident/
  // None controls, and the by-field chips. Folder renames carry "Folder".
  const allItems = useMemo(
    () => [
      ...plans.flatMap((p) =>
        p.ops.map((o) => ({ op_id: o.op_id, confidence: o.confidence, label: opLabel(o) })),
      ),
      ...folderSuggestions.map((f) => ({
        op_id: f.op_id,
        confidence: f.confidence,
        label: "Folder",
      })),
    ],
    [plans, folderSuggestions],
  );
  // Items grouped by display label (Folder / File / Title / …) for the
  // by-field tick chips — "I can see the artist column is always right,
  // tick them all at once".
  const labelGroups = useMemo(() => {
    const m = new Map<string, string[]>();
    for (const it of allItems) {
      const arr = m.get(it.label);
      if (arr) arr.push(it.op_id);
      else m.set(it.label, [it.op_id]);
    }
    return [...m.entries()].sort(
      (a, b) => LABEL_ORDER.indexOf(a[0]) - LABEL_ORDER.indexOf(b[0]),
    );
  }, [allItems]);
  // Sections keyed by folder: seeded with the folders that have a rename
  // (so a tidy-only folder whose files are already clean still shows up),
  // then each track plan dropped into its parent folder's section.
  const folderGroups = useMemo(() => {
    const m = new Map<string, CleanupTrackPlan[]>();
    for (const f of folderSuggestions) if (!m.has(f.path)) m.set(f.path, []);
    for (const p of plans) {
      const f = folderOf(p.path);
      const arr = m.get(f);
      if (arr) arr.push(p);
      else m.set(f, [p]);
    }
    return [...m.entries()].sort((a, b) => a[0].localeCompare(b[0]));
  }, [plans, folderSuggestions]);

  function toggleRule(id: CleanupRuleId) {
    setRules((prev) => {
      const next = new Set(prev);
      if (next.has(id)) next.delete(id);
      else next.add(id);
      return next;
    });
  }

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
      ], r.plans.flatMap((plan) => plan.ops)),
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

  function catalogTagKey(suggestion: CleanupCatalogTagSuggestion): string {
    return `${suggestion.track_id}:${suggestion.analyzer_id}:${suggestion.source_signature}:${suggestion.tag}`;
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

  function toggleOp(opId: string) {
    setTicked((prev) => toggleReviewOperation(prev, opId, plans.flatMap((plan) => plan.ops)));
  }

  function toggleLabel(opIds: string[]) {
    setTicked((prev) => {
      const eligible = selectUnambiguous(opIds, plans.flatMap((plan) => plan.ops));
      const next = new Set(prev);
      const allOn = [...eligible].every((id) => next.has(id));
      for (const id of eligible) {
        if (allOn) next.delete(id);
        else next.add(id);
      }
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
    const editionFields = new Set(["album", "album_artist", "track_no", "disc_no", "year"]);
    const removed = new Set(result.plans.filter((plan) => folderOf(plan.path) === folder).flatMap((plan) => plan.ops)
      .filter((op) => op.rules.includes("catalog_identity") && editionFields.has(op.field ?? "")).map((op) => op.op_id));
    const updated = await updateReview({ ...result, plans: result.plans.map((plan) => {
      if (folderOf(plan.path) !== folder) return plan;
      const catalog = catalogPlanByTrack.get(plan.track_id);
      const choice = catalog?.release_choices?.find((item) => item.id === releaseId);
      return { ...plan, ops: [...plan.ops.filter((op) => !removed.has(op.op_id)), ...(choice?.ops ?? []).map((op) => ({ ...op, ...(catalog ? { review_context: catalogReviewContext(catalog) } : {}) }))] };
    }) }, removed);
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
    let batchId: number | null = null;
    let applied = 0;
    let acceptedTags = 0;
    let tagFailures = 0;
    const skipped: { track_id: number; reason: string }[] = [];
    // Catalog review targets are bound to the metadata signature that produced
    // them. Accept those explicit choices before an embedded-metadata repair
    // changes that signature; accepted operator tags then remain durable.
    for (let i = 0; i < tagTargets.length; i += REVIEW_CHUNK) {
      const chunk = tagTargets.slice(i, i + REVIEW_CHUNK);
      try {
        const review = await assistantApi.reviewAnalysisTagsBulk(chunk, "accepted");
        acceptedTags += review.applied.length;
        tagFailures += review.failures.length;
      } catch (error) {
        tagFailures += chunk.length;
        toast.error(
          "Mood tag review stopped partway",
          error instanceof Error ? error.message : undefined,
        );
        break;
      }
      setProgress({
        done: Math.min(i + REVIEW_CHUNK, tagTargets.length),
        total: ops.length + tagTargets.length,
      });
    }
    try {
      for (let i = 0; i < ops.length; i += APPLY_CHUNK) {
        const chunk = ops.slice(i, i + APPLY_CHUNK);
        const r = await cleanupApi.apply(chunk, batchId, scopeLabel);
        batchId = r.batch_id ?? batchId;
        applied += r.applied;
        skipped.push(...r.skipped);
        setProgress({
          done: tagTargets.length + Math.min(i + APPLY_CHUNK, ops.length),
          total: ops.length + tagTargets.length,
        });
      }
    } catch (e) {
      toast.error(
        "Apply stopped partway",
        `${applied} change${applied === 1 ? "" : "s"} landed before the error: ${
          e instanceof Error ? e.message : "unknown"
        }`,
      );
    }
    setSummary({ applied, skipped, batchId, acceptedTags, tagFailures });
    setStep("done");
    if (applied > 0 || acceptedTags > 0) onApplied();
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
    // A new edition must not inherit another source's field mapping or proposal opt-in.
    setImports(ids.map((track_id) => ({ track_id, fields: { release_mbid: releaseId } })));
    setCheckedIds(ids);
    setScopeType("tracks");
    setPath(folder);
    setUseCatalogs(true);
    setRefreshCatalogs(true);
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

  // --- step bodies ---------------------------------------------------------

  const configureBody = (
    <div className="cleanup-options">
      <section>
        <h3 className="section-label">Where to look</h3>
        <div className="cleanup-scope">
          <label className="cleanup-choice">
            <input
              type="radio"
              name="cleanup-scope"
              checked={scopeType === "all"}
              onChange={() => setScopeType("all")}
            />
            <span>Entire library</span>
          </label>
          <label className="cleanup-choice">
            <input
              type="radio"
              name="cleanup-scope"
              checked={scopeType === "folder"}
              onChange={() => setScopeType("folder")}
            />
            <span>
              Current folder <strong>{path || "(root)"}</strong>
            </span>
            {scopeType === "folder" ? (
              <label className="cleanup-subchoice">
                <input
                  type="checkbox"
                  checked={recursive}
                  onChange={(e) => setRecursive(e.target.checked)}
                />
                <span className="muted">include subfolders</span>
              </label>
            ) : null}
          </label>
          <label className={`cleanup-choice${checkedIds.length === 0 ? " disabled" : ""}`}>
            <input
              type="radio"
              name="cleanup-scope"
              disabled={checkedIds.length === 0}
              checked={scopeType === "tracks"}
              onChange={() => setScopeType("tracks")}
            />
            <span>
              Selected tracks{" "}
              <span className="muted">
                ({checkedIds.length === 0 ? "none ticked in the list" : checkedIds.length})
              </span>
            </span>
          </label>
        </div>
      </section>
      {RULE_GROUPS.map((group) => (
        <section key={group.label}>
          <h3 className="section-label">{group.label}</h3>
          <div className="cleanup-rules">
            {group.rules.map((rule) => (
              <label key={rule.id} className="cleanup-choice">
                <input
                  type="checkbox"
                  checked={rules.has(rule.id)}
                  onChange={() => toggleRule(rule.id)}
                />
                <span>
                  {rule.label}
                  <span className="cleanup-hint muted">{rule.hint}</span>
                </span>
              </label>
            ))}
          </div>
        </section>
      ))}
      <section>
        <h3 className="section-label">Catalog evidence</h3>
        <label className="cleanup-choice cleanup-catalog-choice">
          <input
            type="checkbox"
            checked={useCatalogs}
            onChange={(event) => setUseCatalogs(event.target.checked)}
          />
          <span>
            Identify tracks and retrieve canonical metadata
            <span className="cleanup-hint muted">
              Uses the enabled Sources connectors. Ambiguous matches make no proposal; catalog
              repairs and community mood tags always start unticked. MusicBrainz receives title,
              artist, album, duration, and embedded or imported catalog identifiers; AcoustID receives a local fingerprint and duration;
              Last.fm receives the identified recording ID. Library paths and audio files are
              never uploaded.
            </span>
          </span>
        </label>
        {useCatalogs && <>
          <p className="cleanup-hint muted">Catalog lookup supports up to {MAX_CATALOG_TRACKS} tracks per run. Larger scans still produce local cleanup suggestions.</p>
          <label className="cleanup-choice"><input type="checkbox" checked={refreshCatalogs} onChange={(event) => setRefreshCatalogs(event.target.checked)} />Refresh catalog results, including previous no-match results</label>
          <CleanupEvidenceImport imports={imports} onChange={setImports} />
        </>}
      </section>
      <p className="muted small">
        Nothing is changed yet — the next step shows every proposed fix as a
        diff for you to confirm. File, folder, and embedded-tag changes are
        journaled and can be reverted from History; accepted community mood
        tags remain database-only operator tags.
      </p>
    </div>
  );

  const tickedCount = ticked.size + tickedCatalogTags.size;
  const reviewBody = (
    <fieldset className="cleanup-review-fieldset" disabled={busy}>
      <div className="cleanup-review-controls">
        <button type="button" className="btn-link" disabled={busy} onClick={() => setStep("rejected")}>Rejected suggestions{rejectedIds.size ? ` (${rejectedIds.size} hidden)` : ""}</button>
        <span>
          <strong>{allItems.length}</strong> proposed change{allItems.length === 1 ? "" : "s"} across{" "}
          <strong>{plans.length}</strong> track{plans.length === 1 ? "" : "s"}
          {folderSuggestions.length > 0 ? (
            <>
              {" "}
              and <strong>{folderSuggestions.length}</strong> folder
              {folderSuggestions.length === 1 ? "" : "s"}
            </>
          ) : null}
          {result ? <span className="muted"> (scanned {result.scanned})</span> : null}
        </span>
        <span className="cleanup-review-spacer" />
        <span className="muted small">Choose one value per field. Tick:</span>
        <button
          type="button"
          className="btn-link"
          onClick={() => setTicked(selectUnambiguous(allItems.map((it) => it.op_id), plans.flatMap((plan) => plan.ops)))}
        >
          All unambiguous
        </button>
        <button
          type="button"
          className="btn-link"
          onClick={() =>
            setTicked(
              selectUnambiguous(allItems.filter((it) => it.confidence === "high").map((it) => it.op_id), plans.flatMap((plan) => plan.ops)),
            )
          }
        >
          Confident only
        </button>
        <button type="button" className="btn-link" onClick={() => setTicked(new Set())}>
          None
        </button>
      </div>
      {labelGroups.length > 1 ? (
        <div className="cleanup-review-controls cleanup-label-row">
          <span className="muted small">By field:</span>
          {labelGroups.map(([label, ids]) => {
            const on = ids.every((id) => ticked.has(id));
            return (
              <button
                key={label}
                type="button"
                className="btn-toggle"
                aria-pressed={on}
                title={
                  on
                    ? `Untick all ${ids.length} ${label} change${ids.length === 1 ? "" : "s"}`
                    : `Tick all ${ids.length} ${label} change${ids.length === 1 ? "" : "s"}`
                }
                onClick={() => toggleLabel(ids)}
              >
                {label} <span className="cleanup-chip-count">{ids.length}</span>
              </button>
            );
          })}
        </div>
      ) : null}
      <div className="cleanup-review">
        {folderGroups.map(([folder, group]) => {
          const folderSugg = folderSuggByPath.get(folder);
          return (
          <section key={folder || "(root)"}>
            <h3 className="section-label cleanup-folder">{folder || "(root)"}</h3>
            {enrichmentSummary?.plans.some((plan) => folderOf(plan.path) === folder) && <CleanupEditionTarget folder={folder} disabled={busy} onTarget={(id) => prepareEditionLookup(folder, id)} />}
            {folderSugg ? (
              <div className="cleanup-review-row"><label className="cleanup-op cleanup-op-folder">
                <input
                  type="checkbox"
                  checked={ticked.has(folderSugg.op_id)}
                  onChange={() => toggleOp(folderSugg.op_id)}
                />
                <span className="cleanup-op-kind">Folder</span>
                <span className="cleanup-diff">
                  <span className="cleanup-old">{folderSugg.old}</span>
                  <span className="cleanup-arrow" aria-hidden="true">
                    →
                  </span>
                  <span className="cleanup-new">{folderSugg.new}</span>
                  {folderSugg.confidence === "low" ? (
                    <span
                      className="badge badge-warn cleanup-conf"
                      title={`A guess (${folderSugg.rules.join(", ")}) — verify before ticking`}
                    >
                      guess
                    </span>
                  ) : null}
                </span>
              </label><button type="button" className="btn-link" disabled={busy} onClick={() => void rejectOperation(folderSugg, folderSugg.path)}>Reject folder suggestion</button></div>
            ) : null}
            {group.map((plan) => (
              <div key={plan.track_id} className="cleanup-track">
                <div className="cleanup-track-path" title={plan.path}>
                  {basename(plan.path)}
                </div>
                {plan.ops.map((op) => (
                  <div key={op.op_id} className="cleanup-review-row"><label className="cleanup-op">
                    <input
                      type="checkbox"
                      checked={ticked.has(op.op_id)}
                      onChange={() => toggleOp(op.op_id)}
                    />
                    <span className="cleanup-op-kind">{opLabel(op)}</span>
                    <span className="cleanup-diff">
                      <span className="cleanup-old">
                        <Value value={op.old} />
                      </span>
                      <span className="cleanup-arrow" aria-hidden="true">
                        →
                      </span>
                      <span className="cleanup-new">
                        <Value value={op.new} />
                      </span>
                      <span className="badge cleanup-conf">{op.rules.includes("model_catalog_choice") ? "AI candidate review" : op.rules.includes("catalog_identity") ? "MusicBrainz" : op.rules.includes("imported_metadata") ? "Imported source" : "Local"}</span>
                      {op.confidence === "low" ? (
                        <span
                          className="badge badge-warn cleanup-conf"
                          title={`A guess (${op.rules.join(", ")}) — verify before ticking`}
                        >
                          guess
                        </span>
                      ) : null}
                      {op.verified ? (
                        <span
                          className="badge badge-ok cleanup-conf"
                          title="Value found in MusicBrainz; review whether it belongs to this recording or edition"
                        >
                          catalog value
                        </span>
                      ) : null}
                    </span>
                  </label><button type="button" className="btn-link" disabled={busy} aria-label={`Reject ${opLabel(op)} suggestion for ${basename(plan.path)}`} onClick={() => void rejectOperation(op, plan.path)}>Reject</button></div>
                ))}
                {catalogPlanByTrack.get(plan.track_id) && <CleanupEvidence
                  plan={catalogPlanByTrack.get(plan.track_id)!}
                  edition={editions[folderOf(plan.path)] ?? catalogPlanByTrack.get(plan.track_id)?.identity?.release_mbid ?? ""}
                  onEdition={(id) => void chooseEdition(folderOf(plan.path), id)}
                />}
                {enrichmentJob && catalogPlanByTrack.get(plan.track_id)?.status === "unmatched" && (catalogPlanByTrack.get(plan.track_id)?.candidates?.length ?? 0) > 0 && <CleanupModelReview
                  trackId={plan.track_id} catalogJobId={enrichmentJob.id} onResult={(review) => void acceptModelReview(review)}
                />}
                {plan.notes.map((note) => (
                  <p key={note} className="cleanup-note">
                    <WarnIcon aria-hidden="true" /> {note}
                  </p>
                ))}
              </div>
            ))}
          </section>
          );
        })}
        {catalogTags.length > 0 ? (
          <section className="cleanup-catalog-tag-review">
            <h3 className="section-label cleanup-folder">Database mood tag suggestions</h3>
            <p className="muted small">
              Last.fm terms are shown only when they exactly match a controlled vocabulary name or
              declared alias. Accepted tags become database-only mood tags; they are not embedded in
              the file and are not part of the file-cleanup rollback journal.
            </p>
            {catalogTags.map((suggestion) => {
              const key = catalogTagKey(suggestion);
              const plan = catalogPlanByTrack.get(suggestion.track_id);
              return (
                <label key={key} className="cleanup-op cleanup-catalog-tag-op">
                  <input
                    type="checkbox"
                    checked={tickedCatalogTags.has(key)}
                    onChange={() => toggleCatalogTag(suggestion)}
                  />
                  <span className="cleanup-op-kind">Mood</span>
                  <span className="cleanup-diff">
                    <strong>{suggestion.tag}</strong>
                    <span className="muted">
                      {plan?.identity === null || plan?.identity === undefined
                        ? `track #${suggestion.track_id}`
                        : `${plan.identity.artist} — ${plan.identity.title}`}
                    </span>
                    <span className="badge cleanup-conf">
                      Last.fm {suggestion.count}
                    </span>
                  </span>
                </label>
              );
            })}
          </section>
        ) : null}
      </div>
    </fieldset>
  );

  const applyingBody = (
    <div className="upload-progress cleanup-progress">
      <div className="upload-progress-label">
        Applying — {progress.done} / {progress.total} changes
      </div>
      <progress
        aria-label="Cleanup progress"
        value={progress.total > 0 ? progress.done / progress.total : 0}
        max={1}
      />
    </div>
  );

  const enrichingBody = (
    <div className="cleanup-checking cleanup-enriching">
      <div className="upload-progress cleanup-progress">
        <div className="upload-progress-label">
          {enrichmentJob?.progress_message || "Preparing catalog lookup…"}
        </div>
        <progress
          aria-label="Catalog enrichment progress"
          value={enrichmentJob?.progress_current ?? 0}
          max={Math.max(enrichmentJob?.progress_total ?? 1, 1)}
        />
      </div>
      <p className="muted small">
        MusicBrainz searches are paced to one request per second. AcoustID fingerprints are created
        locally and used only as a fallback. You may leave this screen; the durable job keeps its
        progress and cached track results.
      </p>
    </div>
  );

  const checkingBody = (
    <div className="cleanup-checking">
      <div className="upload-progress cleanup-progress">
        <div className="upload-progress-label">
          Checking names online — {checkProgress.done} / {checkProgress.total}
        </div>
        <progress
          aria-label="Name check progress"
          value={checkProgress.total > 0 ? checkProgress.done / checkProgress.total : 0}
          max={1}
        />
      </div>
      <p className="muted small">
        Unfamiliar artist/album guesses are checked against MusicBrainz (one
        request per second, as their API asks). Each name is looked up once and
        the verdict is remembered — future cleanups reuse it instantly.
      </p>
    </div>
  );

  const doneBody = summary ? (
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
  ) : null;

  const historyBody = <CleanupHistoryPanel onApplied={onApplied} />;
  const rejectedBody = <CleanupRejectedPanel onRestore={restoreProposal} onRecheck={(proposal) => {
    setRestoredReview(false); setPath(proposal.kind === "folder_rename" ? proposal.path : folderOf(proposal.path));
    setCheckedIds(proposal.track_id > 0 ? [proposal.track_id] : []); setScopeType(proposal.track_id > 0 ? "tracks" : "folder");
    setImports([]); setStep("configure");
  }} />;

  // --- footers ---------------------------------------------------------------

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

  const stepBody =
    step === "configure"
      ? configureBody
      : step === "checking"
        ? checkingBody
        : step === "enriching"
          ? enrichingBody
        : step === "review"
          ? reviewBody
          : step === "applying"
            ? applyingBody
            : step === "done"
              ? doneBody
              : step === "rejected" ? rejectedBody : historyBody;

  const body = <>
    {catalogScopeNotice && !busy && (step === "configure" || step === "review") && (
      <p className="cleanup-catalog-summary" role="status">{catalogScopeNotice}</p>
    )}
    {enrichmentJob?.status === "succeeded" && enrichmentSummary !== null && !busy &&
      (step === "configure" || step === "review" || step === "done") && (
      <>
        <div className="cleanup-catalog-summary" role="status">
          <strong>{enrichmentSummary.identified}</strong> identified
          {enrichmentSummary.fingerprinted > 0
            ? ` · ${enrichmentSummary.fingerprinted} needed fingerprinting`
            : ""}
          {enrichmentSummary.unmatched > 0 ? ` · ${enrichmentSummary.unmatched} unmatched` : ""}
          {enrichmentSummary.failed > 0 ? ` · ${enrichmentSummary.failed} failed` : ""}
          {enrichmentSummary.cached > 0 ? ` · ${enrichmentSummary.cached} reused from cache` : ""}
          {incompleteCatalogPlans.length > 0 && (
            <p>
              Catalog evidence is incomplete for {incompleteCatalogPlans.length} track{incompleteCatalogPlans.length === 1 ? "" : "s"}.
              {incompleteUnmatchedCount > 0
                ? ` ${incompleteUnmatchedCount} of the unmatched tracks had incomplete lookups.`
                : ""}
              {" "}Available suggestions remain reviewable. Run the lookup again to retry missing evidence.
            </p>
          )}
        </div>
        <p>
          <button type="button" className="btn-ghost" onClick={downloadCatalogResult}>
            Download catalog results
          </button>
          <span className="muted small"> Saves this run's original proposals and evidence as JSON.</span>
        </p>
        <CleanupCatalogCopy key={enrichmentJob.id} job={enrichmentJob} />
      </>
    )}
    {stepBody}
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
