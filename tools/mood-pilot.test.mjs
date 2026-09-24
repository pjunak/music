import { test } from "node:test";
import assert from "node:assert/strict";
import { mkdtemp, readFile, writeFile, rm } from "node:fs/promises";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { createPilot, freezePilot, scorePilot, parsePilot, serializePilot, main } from "./mood-pilot.mjs";

const vocabulary = { groups: [{ key: "mood", tags: [{ id: "m.calm", name: "calm" }, { id: "m.urgent", name: "urgent" }, { id: "m.sad", name: "sad" }] }, { key: "scene", tags: [{ id: "s.rest", name: "rest" }] }] };
const ids = Array.from({ length: 20 }, (_, index) => index + 1);
const run = (tags = ["calm", "rest"]) => ({ schema_version: "assistant-mood-run-export/v1", run_id: "pilot", track_results: ids.map((track_id) => ({ track_id, tags, source_signature: `source-${track_id}` })) });
function draft() {
  const rows = createPilot(run(), vocabulary);
  Object.assign(rows[0], { annotator: "owner", core_tag_ids: ["m.calm", "m.urgent", "m.sad", "s.rest"] });
  rows.slice(1).forEach((track) => Object.assign(track, { recording_group: `recording-${track.track_id}`, file_reference: `sha256-${track.track_id}`, reviewed: true, listened_intervals: [[0, 120]], labels: { "m.calm": "positive", "m.urgent": "negative", "m.sad": "uncertain", "s.rest": "positive" } }));
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
