import { test } from "node:test";
import assert from "node:assert/strict";
import { createCohort, scorePilot } from "./mood-pilot.mjs";

const ids = Array.from({ length: 30 }, (_, index) => index + 1);
const vocabulary = { groups: [{ key: "mood", tags: [{ name: "calm" }, { name: "urgent" }] }, { key: "scene", tags: [{ name: "rest" }] }] };
const cohort = () => { const result = createCohort(ids); result.tracks.forEach((track) => { track.reviewed = true; track.expected_tags = ["calm", "rest"]; }); return result; };
const run = (tags) => ({ schema_version: "assistant-mood-run-export/v1", track_results: ids.map((track_id) => ({ track_id, tags })) });

test("splits are deterministic and cannot be chosen by input order", () => {
  assert.deepEqual(createCohort(ids), createCohort([...ids].reverse()));
  assert.equal(createCohort(ids).tracks.filter((track) => track.split === "holdout").length, 10);
  assert.throws(() => createCohort(ids.slice(1)));
  assert.throws(() => createCohort([...ids.slice(1), 2]));
});
test("requires listening judgments and treats empty output differently from missing output", () => {
  assert.throws(() => scorePilot(createCohort(ids), run([]), vocabulary), /listening/);
  const empty = scorePilot(cohort(), run([]), vocabulary).splits.holdout.all;
  assert.equal(empty.empty_results, 10); assert.equal(empty.missing_results, 0);
  assert.equal(empty.precision, null); assert.equal(empty.useful_track_coverage, 0);
  assert.equal(empty.meets_proposed_targets, null);
  const missing = scorePilot(cohort(), { ...run([]), track_results: [] }, vocabulary).splits.holdout.all;
  assert.equal(missing.missing_results, 10); assert.equal(missing.empty_results, 0);
});
test("reports per-group mistakes, coverage and held-out results without rewarding excess tags", () => {
  const result = scorePilot(cohort(), run(["calm", "urgent", "rest"]), vocabulary).splits.holdout;
  assert.equal(result.mood.precision, 0.5); assert.equal(result.mood.false_positive_tags, 10);
  assert.equal(result.mood.useful_track_coverage, 1); assert.equal(result.mood.meets_proposed_targets, false);
  assert.equal(result.session_use.precision, 1); assert.equal(result.session_use.meets_proposed_targets, true);
  assert.equal(result.period.precision, null);
});
test("rejects duplicate results and mismatched vocabularies", () => {
  assert.throws(() => scorePilot(cohort(), { ...run([]), track_results: [...run([]).track_results, { track_id: 1, tags: [] }] }, vocabulary), /duplicate/);
  assert.throws(() => scorePilot(cohort(), run(["invented"]), vocabulary), /absent/);
});
