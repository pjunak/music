import { describe, expect, it } from "vitest";
import type { CleanupAnalyzeResult, CleanupEnrichmentResult, CleanupOp } from "@/core/api";
import { mergeEnrichment, selectUnambiguous, toggleReviewOperation } from "./cleanupReview";

const local: CleanupOp = { op_id: "local", track_id: 1, kind: "tag", field: "title", old: "Old", new: "Local title", rules: [], confidence: "high", verified: false };
const catalog: CleanupOp = { ...local, op_id: "catalog", new: "Catalog title", confidence: "low", rules: ["catalog_identity"] };

describe("cleanup review alternatives", () => {
  it("retains local and catalog values and no-operation notes", () => {
    const analysis: CleanupAnalyzeResult = { scanned: 2, folders: [], pending_lookups: [], plans: [{ track_id: 1, path: "a.mp3", ops: [local], notes: [] }] };
    const result: CleanupEnrichmentResult = { schema: "library-cleanup-enrichment/v1", scanned: 2, identified: 1, fingerprinted: 0, unmatched: 1, failed: 0, cached: 0, plans: [
      { schema: "library-cleanup-enrichment/v1", track_id: 1, path: "a.mp3", status: "identified", identity: null, ops: [catalog], tag_suggestions: [], notes: [] },
      { schema: "library-cleanup-enrichment/v1", track_id: 2, path: "b.mp3", status: "unmatched", identity: null, ops: [], tag_suggestions: [], notes: ["Conflicting recording IDs"] },
    ] };
    const merged = mergeEnrichment(analysis, result);
    expect(merged.plans[0].ops).toEqual([local, { ...catalog, review_context: "[null,null,null]" }]);
    expect(merged.plans[1].notes).toEqual(["Conflicting recording IDs"]);
    expect(analysis.plans[0].ops).toEqual([local]);
  });
  it("bulk selection abstains on conflicts, explicit selection replaces alternatives", () => {
    expect(selectUnambiguous(["local", "catalog", "folder"], [local, catalog])).toEqual(new Set(["folder"]));
    expect(toggleReviewOperation(new Set(["local", "folder"]), "catalog", [local, catalog])).toEqual(new Set(["catalog", "folder"]));
    expect(toggleReviewOperation(new Set(["catalog"]), "catalog", [local, catalog])).toEqual(new Set());
    expect(selectUnambiguous(["local", "catalog"], [local, { ...catalog, new: local.new }])).toEqual(new Set(["local"]));
  });
});
