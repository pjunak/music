import { test } from "node:test";
import assert from "node:assert/strict";
import { mkdtemp, readFile, rm, writeFile } from "node:fs/promises";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { createCohort, scorePilot, main } from "./cleanup-pilot.mjs";

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
