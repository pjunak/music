import type { CleanupAnalyzeResult, CleanupEnrichmentResult, CleanupOp } from "@/core/api";

export function mergeEnrichment(local: CleanupAnalyzeResult, enrichment: CleanupEnrichmentResult): CleanupAnalyzeResult {
  const plans = new Map(local.plans.map((plan) => [plan.track_id, plan]));
  for (const catalog of enrichment.plans) {
    const current = plans.get(catalog.track_id);
    plans.set(catalog.track_id, {
      track_id: catalog.track_id,
      path: catalog.path,
      ops: [...(current?.ops ?? []), ...catalog.ops],
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
