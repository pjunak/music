import { cleanupApi } from "@/core/api";
import type { CleanupAnalyzeResult, CleanupEnrichmentResult, CleanupOp, CleanupReviewProposal, CleanupFolderSuggestion, CleanupEnrichmentPlan } from "@/core/api";

export function catalogReviewContext(plan: CleanupEnrichmentPlan): string {
  return JSON.stringify([plan.local_evidence_signature, plan.folder_context_signature, plan.evidence_revision]);
}

export function reviewProposal(op: CleanupOp | CleanupFolderSuggestion, path: string): CleanupReviewProposal {
  const trackOp = "track_id" in op ? op : null;
  return {
    op_id: op.op_id, track_id: trackOp?.track_id ?? 0, kind: trackOp?.kind ?? "folder_rename",
    path, field: trackOp?.field ?? null, old: op.old, new: op.new, rules: op.rules,
    confidence: op.confidence, verified: trackOp?.verified ?? false,
    evidence: trackOp?.evidence ?? null, evidence_context: trackOp?.review_context ?? null,
  };
}

export async function rejectedOperationIds(result: CleanupAnalyzeResult): Promise<Set<string>> {
  const proposals = [...result.plans.flatMap((plan) => plan.ops.map((op) => reviewProposal(op, plan.path))),
    ...result.folders.map((folder) => reviewProposal(folder, folder.path))];
  const rejected = new Set<string>();
  for (let i = 0; i < proposals.length; i += 100) {
    const chunk = proposals.slice(i, i + 100);
    const matches = await cleanupApi.matchRejected(chunk);
    if (matches.length !== chunk.length) throw new Error("The rejected suggestion list could not be checked.");
    matches.forEach((id, index) => { if (id !== null) rejected.add(chunk[index]!.op_id); });
  }
  return rejected;
}

export function mergeEnrichment(local: CleanupAnalyzeResult, enrichment: CleanupEnrichmentResult): CleanupAnalyzeResult {
  const plans = new Map(local.plans.map((plan) => [plan.track_id, plan]));
  for (const catalog of enrichment.plans) {
    const current = plans.get(catalog.track_id);
    plans.set(catalog.track_id, {
      track_id: catalog.track_id,
      path: catalog.path,
      ops: [...(current?.ops ?? []), ...catalog.ops.map((op) => ({ ...op, review_context: catalogReviewContext(catalog) }))],
      notes: [...new Set([...(current?.notes ?? []), ...catalog.notes])],
    });
  }
  return { ...local, plans: [...plans.values()] };
}

function fieldKey(op: CleanupOp): string {
  return `${op.track_id}:${op.kind}:${op.field ?? ""}`;
}

/** Bulk actions leave competing values for an explicit individual choice. */
export function selectUnambiguous(ids: Iterable<string>, ops: CleanupOp[]): Set<string> {
  const values = new Map<string, Set<string>>();
  for (const op of ops) {
    const key = fieldKey(op);
    const group = values.get(key) ?? new Set<string>();
    group.add(JSON.stringify(op.new));
    values.set(key, group);
  }
  const byId = new Map(ops.map((op) => [op.op_id, op]));
  const selected = new Set<string>();
  const seen = new Set<string>();
  for (const id of ids) {
    const op = byId.get(id);
    if (!op) { selected.add(id); continue; } // Folder operations are independent.
    const key = fieldKey(op);
    if (values.get(key)?.size === 1 && !seen.has(key)) {
      selected.add(id);
      seen.add(key);
    }
  }
  return selected;
}

export function toggleReviewOperation(current: Set<string>, id: string, ops: CleanupOp[]): Set<string> {
  const next = new Set(current);
  if (next.delete(id)) return next;
  const selected = ops.find((op) => op.op_id === id);
  if (selected) {
    for (const op of ops) if (fieldKey(op) === fieldKey(selected)) next.delete(op.op_id);
  }
  next.add(id);
  return next;
}
