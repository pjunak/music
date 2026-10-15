import { useMemo } from "react";
import type { Dispatch, ReactNode, SetStateAction } from "react";

import { CleanupEditionComparison } from "@/components/CleanupEditionComparison";
import { CleanupEditionTarget, CleanupEvidence } from "@/components/CleanupEvidence";
import { CleanupModelReview } from "@/components/CleanupModelReview";
import {
  LABEL_ORDER,
  basename,
  catalogTagKey,
  folderOf,
  opLabel,
  selectUnambiguous,
  toggleReviewOperation,
} from "@/components/cleanupReview";
import { WarnIcon } from "@/components/icons";
import type {
  BackgroundJob,
  CleanupCatalogTagSuggestion,
  CleanupEnrichmentResult,
  CleanupFolderSuggestion,
  CleanupModelResult,
  CleanupOp,
  CleanupTrackPlan,
} from "@/core/api";

interface CleanupReviewStepProps {
  busy: boolean;
  scanned: number | null;
  /** Track plans and folder renames with rejected suggestions already hidden. */
  plans: CleanupTrackPlan[];
  folderSuggestions: CleanupFolderSuggestion[];
  rejectedCount: number;
  ticked: Set<string>;
  setTicked: Dispatch<SetStateAction<Set<string>>>;
  catalogTags: CleanupCatalogTagSuggestion[];
  tickedCatalogTags: Set<string>;
  onToggleCatalogTag: (suggestion: CleanupCatalogTagSuggestion) => void;
  enrichmentJob: BackgroundJob | null;
  enrichmentSummary: CleanupEnrichmentResult | null;
  editions: Record<string, string>;
  onShowRejected: () => void;
  onReject: (op: CleanupOp | CleanupFolderSuggestion, path: string) => void;
  onTargetEdition: (folder: string, releaseId: string) => void;
  onChooseEdition: (folder: string, releaseId: string) => void;
  onModelReview: (review: CleanupModelResult) => void;
}

function Value({ value }: { value: string | number | null }) {
  if (value === null || value === "") {
    return <em className="muted">(empty)</em>;
  }
  return <>{String(value)}</>;
}

function GuessBadge({ rules }: { rules: string[] }) {
  return (
    <span
      className="badge badge-warn cleanup-conf"
      title={`A guess (${rules.join(", ")}) — verify before ticking`}
    >
      guess
    </span>
  );
}

function sourceLabel(op: CleanupOp): string {
  if (op.rules.includes("model_catalog_choice")) return "AI candidate review";
  if (op.rules.includes("catalog_identity")) return "MusicBrainz";
  if (op.rules.includes("imported_metadata")) return "Imported source";
  return "Local";
}

/** One proposed change as an old → new diff with its tick box and reject action. */
function ReviewChange({
  className,
  checked,
  onToggle,
  kind,
  previous,
  next,
  badges,
  rejectText,
  rejectLabel,
  busy,
  onReject,
}: {
  className: string;
  checked: boolean;
  onToggle: () => void;
  kind: string;
  previous: ReactNode;
  next: ReactNode;
  badges: ReactNode;
  rejectText: string;
  rejectLabel?: string;
  busy: boolean;
  onReject: () => void;
}) {
  return (
    <div className="cleanup-review-row">
      <label className={className}>
        <input type="checkbox" checked={checked} onChange={onToggle} />
        <span className="cleanup-op-kind">{kind}</span>
        <span className="cleanup-diff">
          <span className="cleanup-old">{previous}</span>
          <span className="cleanup-arrow" aria-hidden="true">
            →
          </span>
          <span className="cleanup-new">{next}</span>
          {badges}
        </span>
      </label>
      <button
        type="button"
        className="btn-link"
        disabled={busy}
        aria-label={rejectLabel}
        onClick={onReject}
      >
        {rejectText}
      </button>
    </div>
  );
}

export function CleanupReviewStep({
  busy,
  scanned,
  plans,
  folderSuggestions,
  rejectedCount,
  ticked,
  setTicked,
  catalogTags,
  tickedCatalogTags,
  onToggleCatalogTag,
  enrichmentJob,
  enrichmentSummary,
  editions,
  onShowRejected,
  onReject,
  onTargetEdition,
  onChooseEdition,
  onModelReview,
}: CleanupReviewStepProps) {
  const ops = useMemo(() => plans.flatMap((plan) => plan.ops), [plans]);
  const folderSuggByPath = useMemo(
    () => new Map(folderSuggestions.map((folder) => [folder.path, folder])),
    [folderSuggestions],
  );
  const catalogPlanByTrack = useMemo(
    () => new Map(enrichmentSummary?.plans.map((plan) => [plan.track_id, plan]) ?? []),
    [enrichmentSummary],
  );

  // Every tickable change (track ops + folder renames) flattened to a common
  // {op_id, confidence, label} shape — drives the count, the All/Confident/
  // None controls, and the by-field chips. Folder renames carry "Folder".
  const allItems = useMemo(
    () => [
      ...ops.map((o) => ({ op_id: o.op_id, confidence: o.confidence, label: opLabel(o) })),
      ...folderSuggestions.map((f) => ({
        op_id: f.op_id,
        confidence: f.confidence,
        label: "Folder",
      })),
    ],
    [ops, folderSuggestions],
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

  function toggleOp(opId: string) {
    setTicked((prev) => toggleReviewOperation(prev, opId, ops));
  }

  function toggleLabel(opIds: string[]) {
    setTicked((prev) => {
      const eligible = selectUnambiguous(opIds, ops);
      const next = new Set(prev);
      const allOn = [...eligible].every((id) => next.has(id));
      for (const id of eligible) {
        if (allOn) next.delete(id);
        else next.add(id);
      }
      return next;
    });
  }

  return (
    <fieldset className="cleanup-review-fieldset" disabled={busy}>
      <div className="cleanup-review-controls">
        <button type="button" className="btn-link" disabled={busy} onClick={onShowRejected}>Rejected suggestions{rejectedCount ? ` (${rejectedCount} hidden)` : ""}</button>
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
          {scanned !== null ? <span className="muted"> (scanned {scanned})</span> : null}
        </span>
        <span className="cleanup-review-spacer" />
        <span className="muted small">Choose one value per field. Tick:</span>
        <button
          type="button"
          className="btn-link"
          onClick={() => setTicked(selectUnambiguous(allItems.map((it) => it.op_id), ops))}
        >
          All unambiguous
        </button>
        <button
          type="button"
          className="btn-link"
          onClick={() =>
            setTicked(
              selectUnambiguous(allItems.filter((it) => it.confidence === "high").map((it) => it.op_id), ops),
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
          const folderCatalogPlans = enrichmentSummary?.plans.filter((plan) => folderOf(plan.path) === folder) ?? [];
          return (
          <section key={folder || "(root)"}>
            <h3 className="section-label cleanup-folder">{folder || "(root)"}</h3>
            {folderCatalogPlans.length > 0 && <CleanupEditionTarget folder={folder} disabled={busy} onTarget={(id) => onTargetEdition(folder, id)} />}
            <CleanupEditionComparison key={enrichmentJob?.id ?? "local"}
              plans={folderCatalogPlans}
              edition={editions[folder]} catalogJobId={enrichmentJob?.id} disabled={busy}
              onEdition={(id) => onChooseEdition(folder, id)} />
            {folderSugg ? (
              <ReviewChange
                className="cleanup-op cleanup-op-folder"
                checked={ticked.has(folderSugg.op_id)}
                onToggle={() => toggleOp(folderSugg.op_id)}
                kind="Folder"
                previous={folderSugg.old}
                next={folderSugg.new}
                badges={folderSugg.confidence === "low" ? <GuessBadge rules={folderSugg.rules} /> : null}
                rejectText="Reject folder suggestion"
                busy={busy}
                onReject={() => onReject(folderSugg, folderSugg.path)}
              />
            ) : null}
            {group.map((plan) => {
              const catalogPlan = catalogPlanByTrack.get(plan.track_id);
              return (
              <div key={plan.track_id} className="cleanup-track">
                <div className="cleanup-track-path" title={plan.path}>
                  {basename(plan.path)}
                </div>
                {plan.ops.map((op) => (
                  <ReviewChange
                    key={op.op_id}
                    className="cleanup-op"
                    checked={ticked.has(op.op_id)}
                    onToggle={() => toggleOp(op.op_id)}
                    kind={opLabel(op)}
                    previous={<Value value={op.old} />}
                    next={<Value value={op.new} />}
                    badges={<>
                      <span className="badge cleanup-conf">{sourceLabel(op)}</span>
                      {op.confidence === "low" ? <GuessBadge rules={op.rules} /> : null}
                      {op.verified ? (
                        <span
                          className="badge badge-ok cleanup-conf"
                          title="Value found in MusicBrainz; review whether it belongs to this recording or edition"
                        >
                          catalog value
                        </span>
                      ) : null}
                    </>}
                    rejectText="Reject"
                    rejectLabel={`Reject ${opLabel(op)} suggestion for ${basename(plan.path)}`}
                    busy={busy}
                    onReject={() => onReject(op, plan.path)}
                  />
                ))}
                {catalogPlan && <CleanupEvidence plan={catalogPlan} />}
                {enrichmentJob && catalogPlan?.status === "unmatched" && (catalogPlan.candidates?.length ?? 0) > 0 && <CleanupModelReview
                  trackId={plan.track_id} catalogJobId={enrichmentJob.id} onResult={onModelReview}
                />}
                {plan.notes.map((note) => (
                  <p key={note} className="cleanup-note">
                    <WarnIcon aria-hidden="true" /> {note}
                  </p>
                ))}
              </div>
              );
            })}
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
                    onChange={() => onToggleCatalogTag(suggestion)}
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
}
