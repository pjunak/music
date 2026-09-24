import { test } from "node:test";
import { execFile } from "node:child_process";
import { promisify } from "node:util";
import assert from "node:assert/strict";
import { mkdtemp, readFile, writeFile, rm } from "node:fs/promises";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { createPilot, freezePilot, scorePilot, comparePilot, parsePilot, serializePilot, main } from "./mood-pilot.mjs";

const vocabulary = { groups: [{ key: "mood", tags: [{ id: "m.calm", name: "calm" }, { id: "m.urgent", name: "urgent" }, { id: "m.sad", name: "sad" }] }, { key: "scene", tags: [{ id: "s.rest", name: "rest" }] }] };
const ids = Array.from({ length: 20 }, (_, index) => index + 1);
const run = (tags = ["calm", "rest"]) => ({ schema_version: "assistant-mood-run-export/v1", run_id: "pilot", track_results: ids.map((track_id) => ({ track_id, tags, source_signature: `source-${track_id}` })) });
function draft() {
  const rows = createPilot(run(), vocabulary);
  Object.assign(rows[0], { annotator: "owner", core_tag_ids: ["m.calm", "m.urgent", "m.sad", "s.rest"] });
  rows.slice(1).forEach((track) => Object.assign(track, { recording_group: `recording-${track.track_id}`, file_reference: `sha256-${track.track_id}`, reviewed: true, blind: true, listened_intervals: [[0, 120]], labels: { "m.calm": "positive", "m.urgent": "negative", "m.sad": "uncertain", "s.rest": "positive" } }));
  return rows;
}
const pilot = () => freezePilot(draft(), vocabulary);

test("one JSONL contract; initialization strips predictions and does not invent recording identity", () => {
  const rows = createPilot(run(), vocabulary);
  assert.deepEqual(parsePilot(serializePilot(rows)), rows);
  assert.deepEqual(rows[1].labels, {});
  assert.equal(rows[1].recording_group, "");
  assert.throws(() => freezePilot(rows, vocabulary), /annotator/);
  assert.throws(() => parsePilot(JSON.stringify({ schema_version: "assistant-mood-pilot/v1", tracks: [] })), /JSONL/);
});

test("transitive duplicate, file and recording groups stay together regardless of input order", () => {
  const rows = draft();
  rows[1].recording_group = rows[2].recording_group;
  rows[2].duplicate_group = rows[3].duplicate_group = "same-version";
  rows[3].file_reference = rows[4].file_reference;
  const frozen = freezePilot(rows, vocabulary);
  assert.deepEqual(frozen, freezePilot([rows[0], ...rows.slice(1).reverse()], vocabulary));
  assert.equal(new Set(frozen.slice(1, 5).map((t) => t.split)).size, 1);
  assert.throws(() => freezePilot(frozen, vocabulary), /already frozen/);
  frozen[1].split = frozen[1].split === "development" ? "confirmation" : "development";
  assert.throws(() => scorePilot(frozen, run(), vocabulary), /partition/);
});

test("optional album/composer separation is transitive; over-grouping fails visibly", () => {
  const rows = draft(); rows[0].separate_by = ["album", "composer"];
  rows[1].album = rows[2].album = "album-a";
  rows[2].composers = rows[3].composers = ["composer-a"];
  const frozen = freezePilot(rows, vocabulary);
  assert.equal(new Set(frozen.slice(1, 4).map((t) => t.split)).size, 1);
  rows.slice(1).forEach((t) => { t.composers = ["only-composer"]; });
  assert.throws(() => freezePilot(rows, vocabulary), /fewer than two/);
});

test("partial labels mask unjudged and uncertain predictions without rewarding excess tags", () => {
  const rows = pilot(); rows.slice(1).forEach((t) => { delete t.labels["s.rest"]; });
  const result = scorePilot(rows, run(["calm", "urgent", "sad", "rest"]), vocabulary);
  const n = result.categories.all.tracks;
  assert.equal(result.categories.mood.precision, 0.5);
  assert.equal(result.categories.all.false_positive_tags, n);
  assert.equal(result.categories.all.uncertain_proposals, n);
  assert.equal(result.categories.all.unjudged_proposals, n);
  assert.equal(result.categories.all.judgment_coverage, 0.5);
  assert.equal(result.categories.all.proposal_judgment_coverage, 0.5);
  assert.equal(result.categories.session_use.precision, null);
  assert.equal(result.per_tag["m.calm"].useful_tags, n);
  assert.deepEqual(result.categories.mood.bootstrap_95.precision, [0.5, 0.5]);
});

test("no output is missing; explicit empty output is abstention with unknown precision", () => {
  const rows = pilot();
  const empty = scorePilot(rows, run([]), vocabulary).categories.all;
  assert.equal(empty.empty_results, empty.tracks); assert.equal(empty.missing_results, 0);
  assert.equal(empty.precision, null); assert.equal(empty.useful_track_coverage, 0);
  const missing = scorePilot(rows, { ...run(), track_results: [] }, vocabulary).categories.all;
  assert.equal(missing.missing_results, missing.tracks); assert.equal(missing.empty_results, 0);
  assert.equal(missing.bootstrap_95.precision, null);
});

test("development scoring never requires or scores untouched confirmation labels", () => {
  const rows = pilot(); rows.slice(1).filter((t) => t.split === "confirmation").forEach((t) => { t.reviewed = false; t.labels = {}; t.listened_intervals = []; });
  assert.equal(scorePilot(rows, run(), vocabulary).split, "development");
  assert.throws(() => scorePilot(rows, run(), vocabulary, "confirmation"), /listening/);
});

test("reports residual overlap, listening limitations and candidate signatures without hiding missing data", () => {
  const rows = draft(); rows.slice(1).forEach((t) => { t.album = "shared"; t.composers = ["shared-composer"]; t.scope = "excerpt"; t.blind = false; });
  const frozen = freezePilot(rows, vocabulary);
  const result = scorePilot(frozen, run(), vocabulary, "confirmation");
  assert.deepEqual(result.residual_overlap, { albums: ["shared"], composers: ["shared-composer"] });
  assert.equal(result.listening.assisted_tracks, result.categories.all.tracks);
  assert.equal(result.listening.excerpt_tracks, result.categories.all.tracks);
  assert.equal(Object.keys(result.run_source_signatures).length, result.categories.all.tracks);
  assert.deepEqual(result, scorePilot(frozen, run(), vocabulary, "confirmation"));
});

test("rejects changed vocabulary, malformed intervals, ambiguous identity and untrusted run rows", () => {
  assert.throws(() => scorePilot(pilot(), run(), { ...vocabulary, revision: 2 }), /Vocabulary changed/);
  const rows = draft(); rows[1].listened_intervals = [[20, 10]];
  assert.throws(() => freezePilot(rows, vocabulary), /Intervals/);
  rows[1].listened_intervals = [[0, 20], [10, 30]];
  assert.throws(() => freezePilot(rows, vocabulary), /Intervals/);
  const duplicate = run(); duplicate.track_results.push(duplicate.track_results[0]);
  assert.throws(() => scorePilot(pilot(), duplicate, vocabulary), /duplicate/);
  assert.throws(() => scorePilot(pilot(), run(["invented"]), vocabulary), /absent/);
  const noSignature = run(); delete noSignature.track_results[0].source_signature;
  assert.throws(() => scorePilot(pilot(), noSignature, vocabulary), /source signature/);
});

test("CLI freezes a draft to a new file and refuses to overwrite private judgments", async () => {
  const directory = await mkdtemp(join(tmpdir(), "music-pilot-"));
  try {
    const source = join(directory, "draft.jsonl"), vocab = join(directory, "vocabulary.json"), target = join(directory, "pilot.jsonl");
    await writeFile(source, serializePilot(draft())); await writeFile(vocab, JSON.stringify(vocabulary));
    await main(["freeze", source, vocab, target]);
    const saved = await readFile(target, "utf8");
    assert.equal(parsePilot(saved)[0].partition_fingerprint.length, 64);
    await assert.rejects(main(["freeze", source, vocab, target]), { code: "EEXIST" });
    assert.equal(await readFile(target, "utf8"), saved);
  } finally { await rm(directory, { recursive: true, force: true }); }
});


test("freeze precedes listening without inventing a blinding answer", () => {
  const rows = draft();
  rows.slice(1).forEach((track) => Object.assign(track, { reviewed: false, blind: null, labels: {}, listened_intervals: [] }));
  const frozen = freezePilot(rows, vocabulary);
  assert.equal(frozen[1].blind, null);
  assert.throws(() => scorePilot(frozen, run(), vocabulary), /Finish independent listening/);
  const selected = frozen.slice(1).filter((track) => track.split === "development");
  selected.forEach((track) => Object.assign(track, { reviewed: true, listened_intervals: [[0, 120]] }));
  assert.throws(() => scorePilot(frozen, run(), vocabulary), /blinding status/);
  selected.forEach((track) => { track.blind = true; });
  assert.equal(scorePilot(frozen, run(), vocabulary).listening.blind_tracks, selected.length);
});

test("recall includes known positives missed by abstentions or unavailable results", () => {
  const rows = pilot(), result = scorePilot(rows, run(["calm"]), vocabulary), n = result.categories.all.tracks;
  assert.equal(result.categories.all.recall, 0.5);
  assert.equal(result.categories.all.missed_positive_tags, n);
  assert.equal(result.categories.mood.recall, 1);
  assert.equal(result.categories.session_use.recall, 0);
  assert.equal(result.per_tag["m.sad"].recall, null);
  assert.deepEqual(result.categories.all.bootstrap_95.recall, [0.5, 0.5]);
  for (const candidate of [run([]), { ...run(), track_results: [] }]) {
    const score = scorePilot(rows, candidate, vocabulary).categories.all;
    assert.equal(score.recall, 0);
    assert.equal(score.missed_positive_tags, n * 2);
  }
});

test("paired comparison reports gains, per-tag losses avoided and category deltas", () => {
  const rows = pilot(), result = comparePilot(rows, run(["calm", "urgent"]), run(), vocabulary);
  const n = result.baseline.categories.all.tracks;
  assert.equal(result.schema_version, "song-mood-comparison/v1");
  assert.equal(result.delta.categories.all.precision, 0.5);
  assert.equal(result.delta.categories.all.recall, 0.5);
  assert.equal(result.delta.categories.all.useful_track_coverage, 0);
  assert.equal(result.delta.per_tag["m.urgent"].false_positive_tags, -n);
  assert.equal(result.delta.per_tag["s.rest"].useful_tags, n);
  assert.equal(result.delta.per_tag["s.rest"].precision, null);
  assert.deepEqual(result.delta.categories.all.bootstrap_95.precision, [0.5, 0.5]);
  assert.deepEqual(result.delta.categories.all.bootstrap_95.recall, [0.5, 0.5]);
  assert.equal(result.result_availability.both_present, n);
  assert.equal(result.tracks_with_improvements, n);
  assert.equal(result.tracks_with_regressions, 0);
  assert.deepEqual(result.differences[0].improvements, ["gained_useful_tag", "avoided_false_positive"]);
  assert.deepEqual(result.differences[0].removed_core_tags, [{ tag_id: "m.urgent", judgment: "negative" }]);
  assert.equal(result.unchanged_tracks, 0);
  assert.equal(result.baseline.partition_fingerprint, result.candidate.partition_fingerprint);
  assert.equal(result.baseline.judgments_fingerprint, result.candidate.judgments_fingerprint);
});

test("paired bootstrap reuses entire recording groups and cancels identical predictions", () => {
  const rows = draft();
  rows.slice(1).forEach((track) => { track.recording_group = "pair-" + Math.floor((track.track_id - 1) / 2); });
  const frozen = freezePilot(rows, vocabulary), baseline = run();
  baseline.track_results.forEach((row) => { row.tags = Math.floor((row.track_id - 1) / 2) % 2 ? ["calm"] : ["urgent"]; });
  const candidate = structuredClone(baseline);
  candidate.track_results.reverse().forEach((row) => { row.source_signature = "new-engine-" + row.track_id; });
  const result = comparePilot(frozen, baseline, candidate, vocabulary);
  const interval = result.baseline.categories.all.bootstrap_95.precision;
  assert.ok(interval[0] < interval[1], "individual candidate uncertainty should be nonzero");
  assert.equal(result.delta.categories.all.bootstrap_95.independent_groups, result.baseline.categories.all.tracks / 2);
  for (const metric of ["precision", "recall", "useful_track_coverage"]) {
    assert.equal(result.delta.categories.all[metric], 0);
    assert.deepEqual(result.delta.categories.all.bootstrap_95[metric], [0, 0]);
  }
  assert.equal(result.differences.length, 0);
  assert.notEqual(result.baseline_run_fingerprint, result.candidate_run_fingerprint);
  assert.deepEqual(result, comparePilot([frozen[0], ...frozen.slice(1).reverse()], baseline, candidate, vocabulary));
});

test("missing results cannot masquerade as avoided false positives", () => {
  const rows = pilot(), baseline = run(["urgent"]);
  const missing = comparePilot(rows, baseline, { ...run(), track_results: [] }, vocabulary);
  assert.equal(missing.delta.categories.all.precision, null);
  assert.equal(missing.delta.categories.all.bootstrap_95.precision, null);
  assert.equal(missing.tracks_with_improvements, 0);
  assert.equal(missing.tracks_with_regressions, missing.baseline.categories.all.tracks);
  assert.deepEqual(missing.differences[0].regressions, ["lost_result"]);
  assert.equal(missing.differences[0].candidate.availability, "missing");
  const abstained = comparePilot(rows, baseline, run([]), vocabulary);
  assert.equal(abstained.candidate.categories.all.missing_results, 0);
  assert.equal(abstained.differences[0].candidate.availability, "abstained");
  assert.deepEqual(abstained.differences[0].improvements, ["avoided_false_positive"]);
  assert.equal(abstained.delta.categories.all.precision, null);
});

test("lost results stay in the paired cohort and mixed changes remain visible", () => {
  const rows = pilot(), selected = rows.slice(1).filter((track) => track.split === "development"), candidate = run(["urgent", "rest"]);
  candidate.track_results = candidate.track_results.filter((row) => row.track_id !== selected[0].track_id);
  const result = comparePilot(rows, run(["calm", "urgent"]), candidate, vocabulary), n = selected.length;
  assert.equal(result.candidate.categories.all.tracks, n);
  assert.equal(result.result_availability.baseline_only, 1);
  assert.equal(result.result_availability.both_present, n - 1);
  assert.equal(result.candidate.categories.all.recall, (n - 1) / (2 * n));
  assert.equal(result.candidate.categories.all.missed_positive_tags, n + 1);
  assert.equal(result.differences[0].classification, "regression");
  assert.deepEqual(result.differences[0].regressions, ["lost_result", "lost_useful_tag"]);
  assert.equal(result.differences[1].classification, "mixed");
  assert.deepEqual(result.differences[1].improvements, ["gained_useful_tag"]);
  assert.deepEqual(result.differences[1].regressions, ["lost_useful_tag"]);
});

test("uncertain, unjudged and non-core tag changes establish no quality gain", () => {
  const rows = draft();
  rows[0].core_tag_ids = ["m.calm", "m.urgent", "m.sad"];
  rows.slice(1).forEach((track) => { delete track.labels["m.calm"]; });
  const result = comparePilot(freezePilot(rows, vocabulary), run([]), run(["calm", "sad", "rest"]), vocabulary);
  assert.equal(result.tracks_with_improvements, 0);
  assert.equal(result.tracks_with_regressions, 0);
  assert.equal(result.differences[0].classification, "changed");
  assert.deepEqual(result.differences[0].added_core_tags, [{ tag_id: "m.calm", judgment: "unjudged" }, { tag_id: "m.sad", judgment: "uncertain" }]);
  assert.deepEqual(result.differences[0].added_non_core_tags, ["s.rest"]);
  assert.equal(result.delta.categories.all.precision, null);
  assert.equal(result.delta.categories.all.recall, null);
});

test("comparison excludes confirmation judgments and extra results until explicitly selected", () => {
  const rows = pilot(), baseline = run(), candidate = run();
  const confirmation = rows.slice(1).filter((track) => track.split === "confirmation");
  for (const track of confirmation) candidate.track_results.find((row) => row.track_id === track.track_id).tags = ["urgent"];
  const outside = { track_id: 9999, tags: ["urgent"], source_signature: "outside-pilot" };
  candidate.track_results.push(outside);
  candidate.usage = { total_tokens: 500 };
  const judged = structuredClone(rows);
  confirmation.forEach((track) => Object.assign(track, { reviewed: false, blind: null, labels: {}, listened_intervals: [] }));
  const result = comparePilot(rows, baseline, candidate, vocabulary);
  assert.equal(result.differences.length, 0);
  assert.equal(result.candidate.usage.total_tokens, 500);
  assert.equal(result.baseline.judgments_fingerprint, result.candidate.judgments_fingerprint);
  assert.throws(() => comparePilot(rows, baseline, candidate, vocabulary, "confirmation"), /Finish independent listening/);
  const opened = comparePilot(judged, baseline, candidate, vocabulary, "confirmation");
  assert.equal(opened.tracks_with_regressions, confirmation.length);
  assert.equal(opened.delta.categories.all.precision, -1);
  assert.equal(opened.result_availability.both_present, confirmation.length);
  assert.throws(() => comparePilot(rows, baseline, candidate, vocabulary, "all"), /explicitly/);
});

test("small groups and sparsely defined paired ratios retain unknown intervals", () => {
  const rows = draft().slice(0, 3), frozen = freezePilot(rows, vocabulary);
  const result = comparePilot(frozen, run(), run(), vocabulary);
  assert.equal(result.delta.categories.all.precision, 0);
  assert.equal(result.delta.categories.all.bootstrap_95.independent_groups, 1);
  assert.equal(result.delta.categories.all.bootstrap_95.replicates, 0);
  assert.equal(result.delta.categories.all.bootstrap_95.precision, null);
  const sparse = pilot(), selected = sparse.slice(1).filter((track) => track.split === "development");
  selected.forEach((track) => { track.labels = {}; });
  selected[0].labels["m.calm"] = "positive";
  const report = comparePilot(sparse, run(["calm"]), run(["calm"]), vocabulary).delta.categories.all;
  assert.equal(report.precision, 0);
  assert.equal(report.bootstrap_95.precision, null);
  assert.ok(report.bootstrap_95.defined_replicates.precision > 0 && report.bootstrap_95.defined_replicates.precision < 900);
});

test("comparison validates both runs and frozen vocabulary before reporting", () => {
  const rows = pilot(), duplicate = run();
  duplicate.track_results.push(duplicate.track_results[0]);
  assert.throws(() => comparePilot(rows, run(), duplicate, vocabulary), /duplicate/);
  assert.throws(() => comparePilot(rows, run(["invented"]), run(), vocabulary), /absent/);
  assert.throws(() => comparePilot(rows, run(), run(), { ...vocabulary, revision: 2 }), /Vocabulary changed/);
  rows[1].file_reference = "different-recording";
  assert.throws(() => comparePilot(rows, run(), run(), vocabulary), /partition/);
});

test("CLI compares private snapshots without modifying them and requires the explicit confirmation flag", async () => {
  const directory = await mkdtemp(join(tmpdir(), "music-pilot-comparison-"));
  try {
    const values = [serializePilot(pilot()), JSON.stringify(run(["calm", "urgent"])), JSON.stringify(run()), JSON.stringify(vocabulary)];
    const paths = ["pilot.jsonl", "baseline.json", "candidate.json", "vocabulary.json"].map((name) => join(directory, name));
    await Promise.all(paths.map((path, index) => writeFile(path, values[index])));
    const command = promisify(execFile);
    const { stdout } = await command(process.execPath, ["tools/mood-pilot.mjs", "compare", ...paths]);
    const result = JSON.parse(stdout);
    assert.equal(result.split, "development");
    assert.equal(result.delta.categories.all.precision, 0.5);
    const confirmation = await command(process.execPath, ["tools/mood-pilot.mjs", "compare", ...paths, "--confirmation"]);
    assert.equal(JSON.parse(confirmation.stdout).split, "confirmation");
    await assert.rejects(command(process.execPath, ["tools/mood-pilot.mjs", "compare", ...paths, "--all"]), /Usage/);
    assert.deepEqual(await Promise.all(paths.map((path) => readFile(path, "utf8"))), values);
  } finally { await rm(directory, { recursive: true, force: true }); }
});
