import assert from "node:assert/strict";
import { createHash } from "node:crypto";
import fs from "node:fs";
import { tmpdir } from "node:os";
import path from "node:path";
import { test } from "node:test";
import type { TestContext } from "node:test";
import { aggregateRecording, buildArtifact, canonicalJson, readCorpusManifest, SCORE_KIND, SCHEMA_VERSION,
  validateStreamedPcm } from "./jev-audio-evidence.mts";
import type { AggregationInput, EvidencePatch, SourceKind, Taxonomy } from "./jev-audio-evidence.mts";

const COUNTS: Record<SourceKind, number> = { instrument: 40, mood_theme: 56, style: 400 };
const HASHES: Record<SourceKind, string> = { instrument: "1".repeat(64), mood_theme: "2".repeat(64), style: "3".repeat(64) };

function temporary(t: TestContext): string {
  const directory = fs.mkdtempSync(path.join(tmpdir(), "jev-audio-evidence-"));
  t.after(() => fs.rmSync(directory, { recursive: true, force: true }));
  return directory;
}

function taxonomies(): Record<SourceKind, Taxonomy> {
  return Object.fromEntries((Object.keys(COUNTS) as SourceKind[]).map(kind => [kind, {
    kind, modelId: `${kind}/1`, modelFile: `${kind}.onnx`, modelSha256: HASHES[kind],
    taxonomyFile: `${kind}.json`, taxonomySha256: HASHES[kind].replace(/./g, kind === "style" ? "4" : "5"),
    labelOrderSha256: HASHES[kind],
    labelCount: COUNTS[kind], metadataName: kind,
    labels: Array.from({ length: COUNTS[kind] }, (_, index) => `${kind}-${index}`),
  }])) as Record<SourceKind, Taxonomy>;
}

function vector(count: number, values: Record<number, number> = {}): number[] {
  return Array.from({ length: count }, (_, index) => values[index] ?? 0);
}

function patch(start: number, end: number, values: Record<number, number>): EvidencePatch {
  return { weight_start_sample: start, weight_end_sample: end, scores: {
    instrument: vector(40, values), mood_theme: vector(56, values), style: vector(400, values),
  } };
}

function input(): AggregationInput {
  return {
    file_sha256: "a".repeat(64), pcm_sha256: "b".repeat(64),
    decoded_samples: 16_000, covered_samples: 16_000, taxonomies: taxonomies(),
    patches: [patch(0, 8_000, { 0: 0.9, 1: 0.8, 2: 0.7, 3: 0.6 }),
      patch(8_000, 16_000, { 0: 0.1, 1: 0.9, 2: 0.8, 3: 0.7 })],
  };
}

void test("time-weighted summaries retain mean, peak, top-three share and opening/ending evidence", () => {
  const recording = aggregateRecording(input());
  assert.equal(recording.duration_seconds, 1);
  assert.equal(recording.covered_seconds, 1);
  assert.match(recording.prediction_sha256, /^[0-9a-f]{64}$/);
  assert.deepEqual(recording.sources.map(source => source.kind), ["instrument", "mood_theme", "style"]);
  for (const source of recording.sources) {
    assert.equal(source.score_kind, SCORE_KIND);
    assert.equal(source.top_rank_limit, 3);
    assert.equal(source.labels.length, COUNTS[source.kind]);
    assert.equal(source.labels[0].mean_score, 0.5);
    assert.equal(source.labels[0].max_score, 0.9);
    assert.equal(source.labels[0].top_rank_fraction, 0.5);
    assert.equal(source.labels[0].opening_score, 0.9);
    assert.equal(source.labels[0].ending_score, 0.1);
    assert.equal(source.labels[1].top_rank_fraction, 1);
    assert.equal(source.labels[2].top_rank_fraction, 1);
    assert.equal(source.labels[3].top_rank_fraction, 0.5);
  }
  const artifact = buildArtifact([recording]);
  assert.equal(artifact.schema_version, SCHEMA_VERSION);
  assert.match(artifact.recording_set_sha256, /^[0-9a-f]{64}$/);
});

void test("exact taxonomy order is preserved and equal scores use taxonomy order for top ranks", () => {
  const value = input();
  value.patches = [patch(0, 16_000, {})];
  value.taxonomies.style.labels[0] = "z-first-in-taxonomy";
  value.taxonomies.style.labels[1] = "a-second-in-taxonomy";
  const source = aggregateRecording(value).sources.find(candidate => candidate.kind === "style");
  assert.ok(source);
  assert.equal(source.labels[0].label, "z-first-in-taxonomy");
  assert.equal(source.labels[1].label, "a-second-in-taxonomy");
  assert.deepEqual(source.labels.slice(0, 5).map(label => label.top_rank_fraction), [1, 1, 1, 0, 0]);
});

void test("wrong score dimensions, missing heads and non-finite or unbounded scores fail", () => {
  for (const bad of [-0.01, 1.01, NaN, Infinity]) {
    const value = input(); value.patches[0].scores.style[0] = bad;
    assert.throws(() => aggregateRecording(value), /bounded scores/);
  }
  const short = input(); short.patches[0].scores.instrument.pop();
  assert.throws(() => aggregateRecording(short), /40 bounded scores/);
  const missing = input();
  delete (missing.patches[0].scores as Partial<Record<SourceKind, number[]>>).mood_theme;
  assert.throws(() => aggregateRecording(missing), /56 bounded scores/);
});

void test("partial, overlapping, gapped and out-of-order coverage fails", () => {
  const variants: EvidencePatch[][] = [
    [patch(0, 8_000, {})],
    [patch(0, 9_000, {}), patch(8_000, 16_000, {})],
    [patch(0, 7_000, {}), patch(8_000, 16_000, {})],
    [patch(8_000, 16_000, {}), patch(0, 8_000, {})],
  ];
  for (const patches of variants) {
    const value = input(); value.patches = patches;
    assert.throws(() => aggregateRecording(value), /partition|coverage/);
  }
  const value = input(); value.covered_samples--;
  assert.throws(() => aggregateRecording(value), /Incomplete audio coverage/);
});

void test("recording and root digests reject altered or duplicate evidence", () => {
  const recording = aggregateRecording(input());
  const changed = structuredClone(recording);
  changed.sources[0].labels[0].mean_score += 0.01;
  assert.throws(() => buildArtifact([changed]), /prediction hash mismatch/);
  assert.throws(() => buildArtifact([recording, recording]), /Duplicate source file hash/);
});

void test("canonical hashing recursively sorts object keys while preserving arrays and numbers", () => {
  const left = { z: 1, a: { y: [3, { b: 2, a: 1 }], x: 0.25 } };
  const right = { a: { x: 0.25, y: [3, { a: 1, b: 2 }] }, z: 1 };
  assert.equal(canonicalJson(left), canonicalJson(right));
  assert.equal(canonicalJson(left), '{"a":{"x":0.25,"y":[3,{"a":1,"b":2}]},"z":1}');
  assert.notEqual(canonicalJson([1, 2]), canonicalJson([2, 1]));
});

void test("corpus manifests bind each source to an exact ordered PCM path and both content hashes", t => {
  const directory = temporary(t), source = path.join(directory, "private-title.m4a"), pcm = path.join(directory, "track-0.f32le");
  const bytes = Buffer.from("bounded private source");
  const pcmBytes = Buffer.from("decoded pcm bytes");
  fs.writeFileSync(source, bytes);
  fs.writeFileSync(pcm, pcmBytes);
  const expected = createHash("sha256").update(bytes).digest("hex");
  const pcmExpected = createHash("sha256").update(pcmBytes).digest("hex");
  const manifest = path.join(directory, "corpus.json");
  const entry = { file: source, bytes: bytes.length, sha256: expected, pcm_file: pcm, pcm_sha256: pcmExpected };
  fs.writeFileSync(manifest, JSON.stringify([entry]));
  assert.deepEqual(readCorpusManifest(manifest, [pcm]), [{ fileSha256: expected, pcmSha256: pcmExpected }]);
  assert.throws(() => readCorpusManifest(manifest, [pcm, pcm]), /count mismatch/);
  const otherPcm = path.join(directory, "track-1.f32le"); fs.writeFileSync(otherPcm, pcmBytes);
  assert.throws(() => readCorpusManifest(manifest, [otherPcm]), /path\/order mismatch/);
  fs.writeFileSync(manifest, JSON.stringify([{ ...entry, pcm_sha256: "f".repeat(64) }]));
  assert.throws(() => readCorpusManifest(manifest, [pcm]), /PCM hash mismatch/);
  fs.writeFileSync(manifest, JSON.stringify([{ ...entry, sha256: "f".repeat(64) }]));
  assert.throws(() => readCorpusManifest(manifest, [pcm]), /Source hash mismatch/);
  fs.writeFileSync(manifest, JSON.stringify([{ ...entry, bytes: bytes.length + 1 }]));
  assert.throws(() => readCorpusManifest(manifest, [pcm]), /size changed/);
  assert.doesNotThrow(() => validateStreamedPcm({ fileSha256: expected, pcmSha256: pcmExpected }, pcmExpected));
  assert.throws(() => validateStreamedPcm({ fileSha256: expected, pcmSha256: pcmExpected }, "f".repeat(64)),
    /Streamed PCM hash mismatch/);
  const artifact = JSON.stringify(buildArtifact([aggregateRecording(input())]));
  assert(!artifact.includes(source));
  assert(!artifact.includes("private-title"));
});
