import { test } from "node:test";
import assert from "node:assert/strict";
import { mkdtemp, readFile, rm, writeFile } from "node:fs/promises";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { createCohort, scorePilot, comparePilot, main } from "./cleanup-pilot.mjs";
import { execFile } from "node:child_process";
import { promisify } from "node:util";

const id = (number) => `00000000-0000-4000-8000-${String(number).padStart(12, "0")}`;
const manifest = Array.from({ length: 10 }, (_, index) => ({ track_id: index + 1, family: `composer-${Math.floor(index / 2)}`, stratum: index % 2 ? "partial album" : "soundtrack" }));
const cohort = () => {
  const result = createCohort(manifest);
  for (const track of result.tracks) {
    track.reviewed = true;
    track.evidence_notes = "Independent booklet and listening review";
    track.expected_recording_mbids = [id(track.track_id)];
    track.expected_release_mbids = [id(100)];
    track.fields = { artist: { current: "Composer", acceptable: ["Composer"] }, title: { current: "01 - Song", acceptable: ["Song"] } };
  }
  return result;
};
const plan = (track_id, status = "identified") => ({ schema: "library-cleanup-enrichment/v1", track_id, status, partial: false,
  identity: status === "identified" ? { recording_mbid: id(track_id), release_mbid: id(100) } : null, candidates: [], ops: [] });
const run = (plans) => ({ schema: "library-cleanup-enrichment/v1", plans });
const op = (field, old, value) => ({ track_id: 1, kind: "tag", field, old, new: value });

test("family splits are deterministic, keep siblings together and reject scope tampering", () => {
  const result = createCohort(manifest);
  assert.deepEqual(result, createCohort([...manifest].reverse()));
  assert.equal(result.tracks.filter((track) => track.split === "holdout").length, 2);
  for (const track of result.tracks) assert.equal(result.tracks.filter((other) => other.family === track.family && other.split !== track.split).length, 0);
  const labeled = cohort();
  labeled.tracks[0].split = labeled.tracks[0].split === "holdout" ? "development" : "holdout";
  assert.throws(() => scorePilot(labeled, run([])), /split/);
  labeled.tracks[0].family = "new family";
  assert.throws(() => scorePilot(labeled, run([])), /manifest/);
  assert.throws(() => createCohort(manifest.map((track) => ({ ...track, family: "one family" }))), /two independent/);
  assert.throws(() => createCohort([...manifest, manifest[0]]), /duplicate/);
});

test("independent labels are mandatory and unknown labels never improve precision", () => {
  assert.throws(() => scorePilot(createCohort(manifest), run([])), /independent/);
  const labeled = cohort();
  labeled.tracks.forEach((track) => { track.expected_recording_mbids = null; track.expected_release_mbids = null; track.fields = {}; });
  const predicted = plan(1);
  predicted.ops = [op("title", "old", "new")];
  const score = scorePilot(labeled, run([predicted])).all;
  assert.equal(score.recording_precision, null);
  assert.equal(score.release_precision, null);
  assert.equal(score.field_precision, null);
  assert.equal(score.unscored_recordings, 1);
  assert.equal(score.unscored_releases, 1);
  assert.equal(score.unscored_fields, 1);
  assert.equal(score.retrieval_coverage, null);
});

test("reports missing, failed, partial and deliberate abstention separately", () => {
  const partial = plan(3); partial.partial = true;
  const result = scorePilot(cohort(), run([plan(1, "unmatched"), plan(2, "failed"), partial]));
  assert.equal(result.all.missing_results, 7);
  assert.equal(result.all.failed_results, 1);
  assert.equal(result.all.abstained_results, 1);
  assert.equal(result.all.partial_results, 1);
  assert.equal(result.all.recording_precision, 1);
  assert.equal(result.all.recording_coverage, 0.1);
  assert.equal(Object.values(result.splits).reduce((sum, split) => sum + split.all.tracks, 0), 10);
});

test("retrieval includes direct identifiers and distinguishes rejected candidates from missed candidates", () => {
  const unresolved = plan(2, "unmatched"); unresolved.candidates = [{ id: id(2) }];
  const wrong = plan(3); wrong.identity.recording_mbid = id(999);
  wrong.identity.release_mbid = id(999);
  const score = scorePilot(cohort(), { result: run([plan(1), unresolved, wrong]) }).all;
  assert.equal(score.known_recordings, 10);
  assert.equal(score.retrieved_recordings, 2);
  assert.equal(score.retrieval_coverage, 0.2);
  assert.equal(score.recording_precision, 0.5);
  assert.equal(score.release_precision, 0.5);
  assert.equal(score.wrong_recordings, 1);
});

test("measures harmful proposals and useful corrections against the same metadata snapshot", () => {
  const predicted = plan(1);
  predicted.ops = [op("artist", "Composer", "Various Artists"), op("title", "01 - Song", "Song"), op("genre", "", "Classical")];
  const score = scorePilot(cohort(), run([predicted])).all;
  assert.equal(score.field_proposals, 3);
  assert.equal(score.unscored_fields, 1);
  assert.equal(score.field_precision, 0.5);
  assert.equal(score.damaged_correct_fields, 1);
  assert.equal(score.useful_corrections, 1);
  assert.equal(score.needed_corrections, 10);
  assert.equal(score.correction_coverage, 0.1);
  predicted.ops[0].old = "Changed after labeling";
  assert.throws(() => scorePilot(cohort(), run([predicted])), /different original metadata snapshots/);
});

test("explicit no-match labels penalize proposed identities, acceptable editions support alternatives", () => {
  const labeled = cohort();
  labeled.tracks[0].expected_recording_mbids = [];
  labeled.tracks[0].expected_release_mbids = [id(100), id(101)];
  const predicted = plan(1); predicted.identity.release_mbid = id(101);
  const result = scorePilot(labeled, run([predicted, plan(11)]));
  assert.equal(result.all.wrong_recordings, 1);
  assert.equal(result.all.correct_releases, 1);
  assert.equal(result.all.known_recordings, 9);
  assert.equal(result.extra_result_tracks, 1);
  assert.equal(result.all.recording_proposals, 1);
});

test("zero-valued source tags can be judged and corrected", () => {
  const labeled = cohort();
  labeled.tracks[0].fields.track_no = { current: 0, acceptable: [1] };
  const predicted = plan(1); predicted.ops = [op("track_no", 0, 1)];
  const score = scorePilot(labeled, run([predicted])).all;
  assert.equal(score.correct_fields, 1);
  assert.equal(score.useful_corrections, 1);
});

test("rejects malformed exports, duplicate IDs, contradictory statuses and duplicate field proposals", () => {
  assert.throws(() => scorePilot(cohort(), { plans: [] }), /enrichment/);
  assert.throws(() => scorePilot(cohort(), run([plan(1), plan(1)])), /duplicate/);
  assert.throws(() => scorePilot(cohort(), run([{ ...plan(1), status: "unmatched" }])), /contradicts/);
  assert.throws(() => scorePilot(cohort(), run([{ ...plan(1), candidates: [{ id: "invalid" }] }])), /candidates/);
  const proposed = op("title", "01 - Song", "Song");
  assert.throws(() => scorePilot(cohort(), run([{ ...plan(1), ops: [proposed, proposed] }])), /duplicate field/);
  const labeled = cohort(); labeled.tracks[0].fields.typo = { current: "", acceptable: ["new"] };
  assert.throws(() => scorePilot(labeled, run([])), /known field/);
});

test("CLI preparation preserves existing files and bounds input size", async () => {
  const directory = await mkdtemp(join(tmpdir(), "cleanup-pilot-"));
  try {
    const source = join(directory, "manifest.json"), target = join(directory, "cohort.json");
    await writeFile(source, JSON.stringify(manifest));
    await main(["init", source, target]);
    assert.deepEqual(JSON.parse(await readFile(target, "utf8")), createCohort(manifest));
    await assert.rejects(main(["init", source, target]), /EEXIST/);
    await writeFile(source, " ".repeat(10 * 1024 * 1024 + 1));
    await assert.rejects(main(["init", source, target]), /10 MiB/);
  } finally { await rm(directory, { recursive: true, force: true }); }
});

const developmentTrack = (labeled) => labeled.tracks.find((track) => track.split === "development");
const trackOp = (track, field, old, value) => ({ ...op(field, old, value), track_id: track.track_id });

test("comparisons isolate development from pending holdout judgments and require an explicit holdout choice", () => {
  const labeled = cohort(), track = developmentTrack(labeled);
  for (const item of labeled.tracks.filter((item) => item.split === "holdout")) {
    item.reviewed = false; item.evidence_notes = "";
  }
  const before = run([plan(track.track_id)]);
  const result = comparePilot(labeled, before, before);
  assert.equal(result.split, "development");
  assert.equal(result.baseline.tracks, 8);
  assert.equal(result.unchanged_tracks, 8);
  assert.deepEqual(result.differences, []);
  assert.equal(result.delta.recording_precision, 0);
  assert.equal(result.delta.field_precision, null);
  assert.throws(() => comparePilot(labeled, before, before, "holdout"), /independent/);
  assert.throws(() => comparePilot(labeled, before, before, "all"), /never combine/);
  const holdout = comparePilot(cohort(), before, before, "holdout");
  assert.equal(holdout.baseline.tracks, 2);
  assert.equal(holdout.baseline.recording_proposals, 0);
  assert.equal(holdout.delta.recording_precision, null);
});

test("paired comparisons expose mixed identity gains and damage to already-correct fields", () => {
  const labeled = cohort(), track = developmentTrack(labeled);
  const before = plan(track.track_id, "unmatched"), after = plan(track.track_id);
  after.ops = [trackOp(track, "artist", "Composer", "Various Artists"), trackOp(track, "title", "01 - Song", "Song")];
  const result = comparePilot(labeled, run([before]), { result: run([after]) });
  assert.equal(result.differences.length, 1);
  const difference = result.differences[0];
  assert.equal(difference.classification, "mixed");
  assert.equal(difference.track_id, track.track_id);
  assert.deepEqual(difference.regressions, ["new_damaged_field"]);
  assert.ok(difference.improvements.includes("recovered_correct_recording"));
  assert.ok(difference.improvements.includes("recovered_useful_correction"));
  assert.equal(result.tracks_with_improvements, 1);
  assert.equal(result.tracks_with_regressions, 1);
  assert.equal(result.delta.damaged_correct_fields, 1);
  assert.equal(result.delta.useful_corrections, 1);
  assert.equal(difference.changed.fields.artist.state, "damaged_correct");
  assert.equal(difference.changed.fields.artist.proposal.new, "Various Artists");
  assert.equal(result.delta.recording_precision, null);
});

test("lost matches and useful corrections are regressions even when aggregate precision stays perfect", () => {
  const labeled = cohort(), tracks = labeled.tracks.filter((track) => track.split === "development").slice(0, 2);
  const first = plan(tracks[0].track_id), second = plan(tracks[1].track_id);
  first.ops = [trackOp(tracks[0], "title", "01 - Song", "Song")];
  const result = comparePilot(labeled, run([first, second]), run([plan(first.track_id, "unmatched"), second]));
  assert.equal(result.baseline.recording_precision, 1);
  assert.equal(result.changed.recording_precision, 1);
  assert.equal(result.delta.correct_recordings, -1);
  assert.equal(result.delta.useful_corrections, -1);
  assert.ok(result.differences[0].regressions.includes("lost_correct_recording"));
  assert.ok(result.differences[0].regressions.includes("lost_useful_correction"));
  assert.ok(result.differences[0].regressions.includes("lost_candidate"));
});

test("missing and failed results never receive credit for avoiding wrong identities or field changes", () => {
  const labeled = cohort(), track = developmentTrack(labeled), wrong = plan(track.track_id);
  wrong.identity.recording_mbid = id(999); wrong.identity.release_mbid = id(999);
  wrong.ops = [trackOp(track, "artist", "Composer", "Wrong"), trackOp(track, "title", "01 - Song", "Wrong")];
  for (const plans of [[], [plan(track.track_id, "failed")]]) {
    const result = comparePilot(labeled, run([wrong]), run(plans));
    assert.equal(result.tracks_with_improvements, 0);
    assert.deepEqual(result.differences[0].regressions, ["lost_result"]);
    assert.deepEqual(result.differences[0].improvements, []);
  }
  const abstained = comparePilot(labeled, run([wrong]), run([plan(track.track_id, "unmatched")]));
  assert.equal(abstained.tracks_with_regressions, 0);
  assert.ok(abstained.differences[0].improvements.includes("avoided_wrong_recording"));
  assert.ok(abstained.differences[0].improvements.includes("preserved_correct_field"));
  assert.ok(abstained.differences[0].improvements.includes("avoided_wrong_field"));
});

test("unknown identities and fields remain unscored when predictions change", () => {
  const labeled = cohort(), track = developmentTrack(labeled);
  track.expected_recording_mbids = null; track.expected_release_mbids = null; track.fields = {};
  const before = plan(track.track_id), after = plan(track.track_id);
  after.identity.recording_mbid = id(999);
  after.ops = [trackOp(track, "artist", "Unknown", "Claimed")];
  const result = comparePilot(labeled, run([before]), run([after]));
  assert.equal(result.differences[0].classification, "changed");
  assert.equal(result.tracks_with_improvements, 0);
  assert.equal(result.tracks_with_regressions, 0);
  assert.equal(result.changed.unscored_fields, 1);
  assert.equal(result.delta.recording_precision, null);
});

test("candidate recovery, partial results, acceptable variants and field damage stay distinct", () => {
  const labeled = cohort(), track = developmentTrack(labeled);
  track.fields.artist.acceptable.push("Composer Alias");
  const before = plan(track.track_id, "unmatched"), retrieved = { ...before, candidates: [{ id: id(track.track_id) }], partial: true };
  const candidates = comparePilot(labeled, run([before]), run([retrieved])).differences[0];
  assert.equal(candidates.classification, "mixed");
  assert.deepEqual(candidates.improvements, ["recovered_candidate"]);
  assert.deepEqual(candidates.regressions, ["became_partial"]);
  const wrong = plan(track.track_id), alternate = plan(track.track_id);
  wrong.ops = [trackOp(track, "artist", "Composer", "Wrong")];
  alternate.ops = [trackOp(track, "artist", "Composer", "Composer Alias")];
  const repaired = comparePilot(labeled, run([wrong]), run([alternate])).differences[0];
  assert.equal(repaired.classification, "improvement");
  assert.deepEqual(repaired.improvements, ["avoided_damaged_field"]);
  assert.equal(comparePilot(labeled, run([plan(track.track_id)]), run([alternate])).differences[0].regressions[0], "new_unnecessary_field_change");
});

test("comparison validates both metadata snapshots, normalizes IDs and does not modify its inputs", () => {
  const labeled = cohort(), track = developmentTrack(labeled), before = plan(track.track_id), after = plan(track.track_id);
  before.identity.recording_mbid = "abcdefab-abcd-4000-8000-000000000001";
  after.identity.recording_mbid = before.identity.recording_mbid.toUpperCase();
  track.expected_recording_mbids = [before.identity.recording_mbid];
  const input = JSON.stringify([labeled, before, after]);
  assert.deepEqual(comparePilot(labeled, run([before]), run([after])).differences, []);
  assert.equal(JSON.stringify([labeled, before, after]), input);
  before.ops = [trackOp(track, "artist", "Incorrect snapshot", "Wrong")];
  assert.throws(() => comparePilot(labeled, run([before]), run([after])), /different original metadata snapshots/);
  assert.throws(() => comparePilot(labeled, run([after]), run([before])), /different original metadata snapshots/);
});

test("CLI comparison reads retained exports, produces JSON and leaves input files unchanged", async () => {
  const directory = await mkdtemp(join(tmpdir(), "cleanup-compare-"));
  try {
    const inputs = [cohort(), run([]), run([])];
    const paths = ["cohort.json", "baseline.json", "changed.json"].map((name) => join(directory, name));
    await Promise.all(paths.map((path, i) => writeFile(path, JSON.stringify(inputs[i]))));
    const { stdout } = await promisify(execFile)(process.execPath, ["tools/cleanup-pilot.mjs", "compare", ...paths]);
    assert.deepEqual(JSON.parse(stdout), comparePilot(...inputs));
    const holdout = await promisify(execFile)(process.execPath, ["tools/cleanup-pilot.mjs", "compare", ...paths, "holdout"]);
    assert.equal(JSON.parse(holdout.stdout).split, "holdout");
    for (const [i, path] of paths.entries()) assert.deepEqual(JSON.parse(await readFile(path, "utf8")), inputs[i]);
    await assert.rejects(promisify(execFile)(process.execPath, ["tools/cleanup-pilot.mjs", "compare", ...paths, "all"]), /never combine/);
  } finally { await rm(directory, { recursive: true, force: true }); }
});
