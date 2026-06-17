import { useMemo, useRef, useState } from "react";
import type { ChangeEvent } from "react";

import { confirmDialog } from "@/components/confirmDialog";
import { EmptyState } from "@/components/EmptyState";
import { WarnIcon } from "@/components/icons";
import { Modal } from "@/components/Modal";
import { cleanupApi } from "@/core/api";
import type {
  CleanupAnalyzeResult,
  CleanupBatchSummary,
  CleanupFolderSuggestion,
  CleanupOp,
  CleanupOpIn,
  CleanupRuleId,
  CleanupScope,
  CleanupTrackPlan,
} from "@/core/api";
import { toast } from "@/core/toast";

/** Library cleanup — "find and fix common rip/download residue".
 *
 *  Four-step flow inside one modal: configure (scope + rules) → review
 *  (every proposed change as an old → new diff with its own checkbox;
 *  low-confidence guesses start unticked) → apply (chunked, real progress
 *  bar, every applied change journaled server-side) → done (counts, skips,
 *  journal download). A History view lists past runs with one-click revert,
 *  plus revert-from-file for a downloaded journal. Nothing is written
 *  without an explicit Apply on the reviewed diff.
 */

type ScopeType = "all" | "folder" | "tracks";
type Step = "configure" | "checking" | "review" | "applying" | "done" | "history";

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

function opLabel(op: CleanupOp): string {
  if (op.kind === "rename") return "File";
  switch (op.field) {
    case "title":
      return "Title";
    case "artist":
      return "Artist";
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
const LABEL_ORDER = ["Folder", "File", "Title", "Artist", "Album", "Track #", "Disc #", "Year"];

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

export function CleanupDialog({
  path,
  checkedIds,
  onClose,
  onApplied,
}: {
  /** Currently-browsed folder ("" = music root). */
  path: string;
  /** Ticked track ids from the Library list — offered as a scope. */
  checkedIds: number[];
  onClose: () => void;
  /** Called whenever anything was written (apply or revert) so the host
   *  view refreshes its tree + track list. */
  onApplied: () => void;
}) {
  const [step, setStep] = useState<Step>("configure");
  const [scopeType, setScopeType] = useState<ScopeType>(
    checkedIds.length > 0 ? "tracks" : path ? "folder" : "all",
  );
  const [recursive, setRecursive] = useState(true);
  const [rules, setRules] = useState<Set<CleanupRuleId>>(new Set(DEFAULT_RULES));
  const [busy, setBusy] = useState(false);
  const [result, setResult] = useState<CleanupAnalyzeResult | null>(null);
  const [ticked, setTicked] = useState<Set<string>>(new Set());
  const [progress, setProgress] = useState({ done: 0, total: 0 });
  const [checkProgress, setCheckProgress] = useState({ done: 0, total: 0 });
  // Set by the Skip button (or closing the dialog) to stop further lookup
  // chunks; whatever resolved so far still feeds the re-analysis.
  const skipCheckRef = useRef(false);
  const [summary, setSummary] = useState<{
    applied: number;
    skipped: { track_id: number; reason: string }[];
    batchId: number | null;
  } | null>(null);
  const [batches, setBatches] = useState<CleanupBatchSummary[] | null>(null);

  const scope: CleanupScope =
    scopeType === "tracks"
      ? { type: "tracks", track_ids: checkedIds }
      : scopeType === "folder"
        ? { type: "folder", path, recursive }
        : { type: "all" };

  const scopeLabel =
    scopeType === "tracks"
      ? `${checkedIds.length} selected track${checkedIds.length === 1 ? "" : "s"}`
      : scopeType === "folder"
        ? `folder “${path || "(root)"}”${recursive ? "" : " (no subfolders)"}`
        : "entire library";

  const plans: CleanupTrackPlan[] = useMemo(() => result?.plans ?? [], [result]);
  const folderSuggestions: CleanupFolderSuggestion[] = useMemo(
    () => result?.folders ?? [],
    [result],
  );
  const folderSuggByPath = useMemo(() => {
    const m = new Map<string, CleanupFolderSuggestion>();
    for (const f of folderSuggestions) m.set(f.path, f);
    return m;
  }, [folderSuggestions]);

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

  function prepareReview(r: CleanupAnalyzeResult): boolean {
    if (r.plans.length === 0 && r.folders.length === 0) {
      toast.success(
        "Nothing to clean",
        `Scanned ${r.scanned} track${r.scanned === 1 ? "" : "s"} — no issues matched the enabled rules.`,
      );
      return false;
    }
    setResult(r);
    // High-confidence suggestions start ticked; guesses (including folder
    // rebuilds) start unticked so a quick "Apply" only commits the safe set.
    setTicked(
      new Set([
        ...r.plans.flatMap((p) =>
          p.ops.filter((o) => o.confidence === "high").map((o) => o.op_id),
        ),
        ...r.folders.filter((f) => f.confidence === "high").map((f) => f.op_id),
      ]),
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
    setBusy(true);
    try {
      let r = await cleanupApi.analyze(scope, [...rules]);
      if (r.pending_lookups.length > 0) {
        // One more clue source: settle unknown artist-vs-album names
        // online, then re-analyze with the verdicts folded in.
        await checkNamesOnline(r.pending_lookups);
        r = await cleanupApi.analyze(scope, [...rules]);
      }
      if (!prepareReview(r)) setStep("configure");
    } catch (e) {
      toast.error("Analysis failed", e instanceof Error ? e.message : undefined);
      setStep("configure");
    } finally {
      setBusy(false);
    }
  }

  function toggleOp(opId: string) {
    setTicked((prev) => {
      const next = new Set(prev);
      if (next.has(opId)) next.delete(opId);
      else next.add(opId);
      return next;
    });
  }

  function toggleLabel(opIds: string[]) {
    // Header-checkbox semantics: if every op of this field is ticked,
    // untick them all; otherwise tick them all.
    setTicked((prev) => {
      const next = new Set(prev);
      const allOn = opIds.every((id) => next.has(id));
      for (const id of opIds) {
        if (allOn) next.delete(id);
        else next.add(id);
      }
      return next;
    });
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
    if (ops.length === 0) return;
    setStep("applying");
    setProgress({ done: 0, total: ops.length });
    let batchId: number | null = null;
    let applied = 0;
    const skipped: { track_id: number; reason: string }[] = [];
    try {
      for (let i = 0; i < ops.length; i += APPLY_CHUNK) {
        const chunk = ops.slice(i, i + APPLY_CHUNK);
        const r = await cleanupApi.apply(chunk, batchId, scopeLabel);
        batchId = r.batch_id ?? batchId;
        applied += r.applied;
        skipped.push(...r.skipped);
        setProgress({ done: Math.min(i + APPLY_CHUNK, ops.length), total: ops.length });
      }
    } catch (e) {
      toast.error(
        "Apply stopped partway",
        `${applied} change${applied === 1 ? "" : "s"} landed before the error: ${
          e instanceof Error ? e.message : "unknown"
        }`,
      );
    }
    setSummary({ applied, skipped, batchId });
    setStep("done");
    if (applied > 0) onApplied();
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

  async function openHistory() {
    setStep("history");
    setBatches(null);
    try {
      setBatches(await cleanupApi.batches());
    } catch (e) {
      toast.error("Couldn't load history", e instanceof Error ? e.message : undefined);
      setBatches([]);
    }
  }

  function reportRevert(reverted: number, skipped: { reason: string }[]) {
    if (skipped.length === 0) {
      toast.success(`Reverted ${reverted} change${reverted === 1 ? "" : "s"}`);
      return;
    }
    const sample = skipped
      .slice(0, 3)
      .map((s) => s.reason)
      .join("\n");
    toast.warn(
      `Reverted ${reverted}, skipped ${skipped.length}`,
      `${sample}${skipped.length > 3 ? `\n…and ${skipped.length - 3} more` : ""}`,
    );
  }

  async function revertBatch(b: CleanupBatchSummary) {
    const ok = await confirmDialog({
      title: `Revert cleanup run #${b.id}?`,
      body:
        `${b.item_count} change${b.item_count === 1 ? "" : "s"} (${b.scope_label || "no label"}) ` +
        "will be undone — renames restored, tags set back. Files changed again since are skipped, not clobbered.",
      tone: "danger",
      confirmLabel: "Revert",
    });
    if (!ok) return;
    try {
      const r = await cleanupApi.revertBatch(b.id);
      reportRevert(r.reverted, r.skipped);
      onApplied();
      setBatches(await cleanupApi.batches());
    } catch (e) {
      toast.error("Revert failed", e instanceof Error ? e.message : undefined);
    }
  }

  function onJournalFilePick(e: ChangeEvent<HTMLInputElement>) {
    const file = e.target.files?.[0];
    e.target.value = "";
    if (!file) return;
    void (async () => {
      let items: unknown[];
      try {
        const parsed: unknown = JSON.parse(await file.text());
        const maybe =
          Array.isArray(parsed) ? parsed : (parsed as { items?: unknown[] })?.items;
        if (!Array.isArray(maybe) || maybe.length === 0) {
          throw new Error("no journal items found in the file");
        }
        items = maybe;
      } catch (err) {
        toast.error(
          "Not a cleanup journal",
          err instanceof Error ? err.message : undefined,
        );
        return;
      }
      const ok = await confirmDialog({
        title: "Revert from journal file?",
        body: `${items.length} recorded change${items.length === 1 ? "" : "s"} from “${file.name}” will be undone where the files still match.`,
        tone: "danger",
        confirmLabel: "Revert",
      });
      if (!ok) return;
      try {
        const r = await cleanupApi.revertJournal(items);
        reportRevert(r.reverted, r.skipped);
        onApplied();
      } catch (err) {
        toast.error("Revert failed", err instanceof Error ? err.message : undefined);
      }
    })();
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
      <p className="muted small">
        Nothing is changed yet — the next step shows every proposed fix as a
        diff for you to confirm, and anything applied is journaled and can be
        reverted from History.
      </p>
    </div>
  );

  const tickedCount = ticked.size;
  const reviewBody = (
    <>
      <div className="cleanup-review-controls">
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
        <span className="muted small">Tick:</span>
        <button
          type="button"
          className="btn-link"
          onClick={() => setTicked(new Set(allItems.map((it) => it.op_id)))}
        >
          All
        </button>
        <button
          type="button"
          className="btn-link"
          onClick={() =>
            setTicked(
              new Set(allItems.filter((it) => it.confidence === "high").map((it) => it.op_id)),
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
            {folderSugg ? (
              <label className="cleanup-op cleanup-op-folder">
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
              </label>
            ) : null}
            {group.map((plan) => (
              <div key={plan.track_id} className="cleanup-track">
                <div className="cleanup-track-path" title={plan.path}>
                  {basename(plan.path)}
                </div>
                {plan.ops.map((op) => (
                  <label key={op.op_id} className="cleanup-op">
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
                          title="Name verified against MusicBrainz"
                        >
                          verified
                        </span>
                      ) : null}
                    </span>
                  </label>
                ))}
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
      </div>
    </>
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
          Everything applied is journaled as run #{summary.batchId} — download it
          for safekeeping, or revert it later from History.
        </p>
      ) : null}
    </div>
  ) : null;

  const historyBody = (
    <div className="cleanup-history">
      {batches === null ? (
        <p className="muted small">Loading…</p>
      ) : batches.length === 0 ? (
        <EmptyState title="No cleanup runs yet">
          Applied cleanup runs appear here with their full change journal —
          each can be downloaded as JSON or reverted.
        </EmptyState>
      ) : (
        batches.map((b) => (
          <div key={b.id} className="cleanup-batch-row">
            <div className="cleanup-batch-main">
              <span>
                <strong>#{b.id}</strong> · {new Date(b.created_at).toLocaleString()}
              </span>
              <span className="muted small">
                {b.item_count} change{b.item_count === 1 ? "" : "s"}
                {b.scope_label ? ` · ${b.scope_label}` : ""}
              </span>
            </div>
            {b.reverted_at !== null ? (
              <span className="badge">reverted</span>
            ) : (
              <button type="button" onClick={() => void revertBatch(b)}>
                Revert
              </button>
            )}
            <button
              type="button"
              className="btn-ghost"
              onClick={() => void downloadJournal(b.id)}
            >
              Download
            </button>
          </div>
        ))
      )}
      <label className="cleanup-journal-upload">
        <input type="file" accept="application/json,.json" hidden onChange={onJournalFilePick} />
        <span className="btn-link">Revert from a downloaded journal file…</span>
      </label>
    </div>
  );

  // --- footers ---------------------------------------------------------------

  const footer =
    step === "configure" ? (
      <>
        <button type="button" className="btn-ghost" onClick={() => void openHistory()}>
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
          disabled={tickedCount === 0}
          onClick={() => void runApply()}
        >
          Apply {tickedCount} change{tickedCount === 1 ? "" : "s"}
        </button>
      </>
    ) : step === "done" ? (
      <>
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
    ) : step === "history" ? (
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
    ) : undefined; // applying: no actions — let it finish

  const titles: Record<Step, string> = {
    configure: "Clean up library",
    checking: "Checking names online",
    review: `Review proposed changes — ${scopeLabel}`,
    applying: "Applying changes",
    done: "Cleanup applied",
    history: "Cleanup history",
  };

  function closeDialog() {
    // Closing mid-check just stops further lookups; resolved verdicts are
    // already cached and benefit the next run.
    skipCheckRef.current = true;
    onClose();
  }

  return (
    <Modal
      title={titles[step]}
      ariaLabel="Library cleanup"
      className="modal-cleanup"
      onClose={step === "applying" ? () => undefined : closeDialog}
      footer={footer}
      closeButton={step !== "applying"}
    >
      {step === "configure"
        ? configureBody
        : step === "checking"
          ? checkingBody
          : step === "review"
            ? reviewBody
            : step === "applying"
              ? applyingBody
              : step === "done"
                ? doneBody
                : historyBody}
    </Modal>
  );
}
