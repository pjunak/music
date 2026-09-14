import { describe, expect, it } from "vitest";
import type { CleanupAnalyzeResult, CleanupEnrichmentResult, CleanupEnrichmentPlan, CleanupReleaseChoice, CleanupOp } from "@/core/api";
import { mergeEnrichment, replaceEdition, selectUnambiguous, toggleReviewOperation } from "./cleanupReview";

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

function edition(id: string): CleanupReleaseChoice {
  return { id, title: "Album", artist: "Artist", date: "2023-08-03", country: "XW", barcode: null, catalog_numbers: [],
    assignment: { classification: "album", considered: 1, matched: 1, unmatched_tracks: [], unmatched_slots: [] },
    ops: ["release_date", "original_release_date", "album", "future_edition_field"].map((field) => ({ ...catalog, op_id: `edition:1:${id}:${field}`, field, new: id })) };
}

it("replaces edition operations on repeated switching and clearing, preserving other evidence", () => {
  const choices = [edition("a"), edition("b")];
  const imported = { ...local, op_id: "imported-date", field: "release_date", rules: ["imported_metadata"] };
  const artist = { ...catalog, op_id: "catalog-artist", field: "artist" };
  const elsewhere = { track_id: 2, path: "Other/song.mp3", ops: [catalog], notes: [] };
  const catalogs: CleanupEnrichmentPlan[] = [{ schema: "library-cleanup-enrichment/v1", track_id: 1, path: "Album/song.mp3", status: "identified", identity: null, ops: [], notes: [], tag_suggestions: [], release_choices: choices }];
  let result: CleanupAnalyzeResult = { scanned: 2, folders: [], pending_lookups: [], plans: [{ track_id: 1, path: "Album/song.mp3", notes: [],
    ops: [local, imported, artist, ...choices[0].ops, ...choices[0].ops] }, elsewhere] };
  for (let i = 0; i < 20; i++) {
    const choice = choices[i % 2];
    const replacement = replaceEdition(result, catalogs, "Album", choice.id);
    result = replacement.result;
    expect(result.plans[0].ops).toHaveLength(7);
    expect(result.plans[0].ops.slice(0, 3)).toEqual([local, imported, artist]);
    expect(result.plans[0].ops.slice(3).map((op) => op.op_id)).toEqual(choice.ops.map((op) => op.op_id));
    expect(result.plans[1]).toBe(elsewhere);
    expect(replacement.removed.size).toBe(4);
  }
  expect(replaceEdition(result, catalogs, "Album", "").result.plans[0].ops).toEqual([local, imported, artist]);
});
