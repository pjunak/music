import assert from "node:assert/strict";
import { createHash } from "node:crypto";
import fs from "node:fs";
import { tmpdir } from "node:os";
import path from "node:path";
import { test } from "node:test";
import type { TestContext } from "node:test";
import { frameCount, MAX_SAMPLES, patchSamples } from "./effnet-reference.mts";
import type { ReferenceRuntime, ReferenceScores } from "./effnet-reference.mts";
import { generateStreamReference, patchSchedule, PcmFrames, streamTrack, TrackSummary } from "./effnet-stream.mts";

function temporary(t: TestContext): string {
  const root = fs.mkdtempSync(path.join(tmpdir(), "effnet-stream-"));
  t.after(() => fs.rmSync(root, { recursive: true, force: true }));
  return root;
}
function pcm(root: string, values: Float32Array | number[], name = "audio.pcm") {
  const bytes = Buffer.alloc(values.length * 4);
  values.forEach((value, i) => bytes.writeFloatLE(value, i * 4));
  const file = path.join(root, name);
  fs.writeFileSync(file, bytes);
  return { file, bytes };
}
function scores(value: number): ReferenceScores {
  return { embeddings: Array(1280).fill(value), mood: Array(56).fill(value), instrument: Array(40).fill(value) };
}
const runtime: ReferenceRuntime = { header: {}, transform: (frame: Float32Array) => new Float32Array(96).fill(frame[256]), infer: async () => scores(0.5) };

void test("all patch supports and non-overlapping weights cover short, aligned and partial endings", () => {
  for (const size of [32257, 32768, 32769, 100000, 100001, MAX_SAMPLES]) {
    const patches = [...patchSchedule(size)], last = frameCount(size) - 128, ending = patches.at(-1);
    const expected: number[] = [];
    assert.ok(patches[0]); assert.ok(ending);
    for (let start = 0; start <= last; start += 62) expected.push(start);
    if (expected.at(-1) !== last) expected.push(last);
    assert.deepEqual(patches.map(patch => patch.frame_start), expected);
    assert.equal(patches[0].weight_start_sample, 0);
    assert.equal(ending.weight_end_sample, size);
    let covered = 0;
    patches.forEach((patch, i) => {
      assert.equal(patch.weight_start_sample, covered);
      assert(patch.weight_end_sample > covered);
      assert(patch.weight_start_sample >= patch.valid_start_sample);
      assert(patch.weight_end_sample <= patch.valid_end_sample);
      if (i) assert(patch.valid_start_sample <= patches[i - 1].valid_end_sample);
      covered = patch.weight_end_sample;
    });
    assert.equal(covered, size);
    assert.equal(ending.valid_end_sample, size);
    assert(patches.length <= 907);
  }
  assert.throws(() => [...patchSchedule(32256)], /too short/);
});

void test("time ownership matches the nearest patch center including the near-duplicate ending", () => {
  const size = 100001, patches = [...patchSchedule(size)], weights = Array(patches.length).fill(0);
  // Independent sample-by-sample oracle, not the scheduler's midpoint expression.
  for (let sample = 0; sample < size; sample++) {
    let winner = 0, distance = Infinity;
    patches.forEach((patch, i) => {
      const center = patch.frame_start * 256 + 16256;
      if (Math.abs(sample + 0.5 - center) < distance) {
        winner = i; distance = Math.abs(sample + 0.5 - center);
      }
    });
    weights[winner]++;
  }
  assert.deepEqual(patches.map(p => p.weight_end_sample - p.weight_start_sample), weights);
});

void test("incremental frames match independent direct sample indexing and hash the consumed bytes", t => {
  const root = temporary(t);
  for (const size of [32257, 32768, 32769, 100001]) {
    const samples = Float32Array.from({ length: size }, (_, i) => i % 257 === 0 ? Math.fround(i / size - 0.5) : 0);
    const { file, bytes } = pcm(root, samples), reader = new PcmFrames(file);
    let count = 0, buffer: ArrayBufferLike | undefined;
    try {
      for (const actual of reader.frames()) {
        if (buffer) assert.equal(actual.buffer, buffer);
        buffer = actual.buffer;
        const start = count++ * 256 - 256;
        assert.deepEqual(Array.from(actual), Array.from({ length: 512 }, (_, i) => samples[start + i] ?? 0));
      }
      assert.equal(count, frameCount(size)); assert(reader.finished);
      assert.equal(reader.digest, createHash("sha256").update(bytes).digest("hex"));
    } finally { reader.close(); }
    assert.equal(reader.fd, undefined);
  }
});

void test("invalid lengths, short audio and nonfinite PCM fail without a completed stream", t => {
  const root = temporary(t), file = path.join(root, "invalid.pcm");
  for (const size of [0, 1, 32256 * 4, MAX_SAMPLES * 4 + 4]) {
    const fd = fs.openSync(file, "w"); fs.ftruncateSync(fd, size); fs.closeSync(fd);
    assert.throws(() => new PcmFrames(file), /PCM|too short/);
  }
  const samples = new Float32Array(40000); samples[30000] = NaN;
  pcm(root, samples, "invalid.pcm");
  const reader = new PcmFrames(file);
  try { assert.throws(() => [...reader.frames()], /Non-finite/); assert(!reader.finished); }
  finally { reader.close(); }
});

void test("streaming detects growth, truncation and same-size replacement", t => {
  const root = temporary(t);
  for (const mutation of ["grow", "truncate", "replace"]) {
    const { file } = pcm(root, new Float32Array(40000));
    const reader = new PcmFrames(file), frames = reader.frames();
    frames.next();
    if (mutation === "grow") fs.appendFileSync(file, Buffer.alloc(4));
    if (mutation === "truncate") fs.truncateSync(file, 30000);
    if (mutation === "replace") {
      fs.renameSync(file, file + ".old");
      pcm(root, new Float32Array(40000));
    }
    try {
      assert.throws(() => { while (!frames.next().done) { /* Consume until the mutation is detected. */ } }, /PCM (grew|changed|was truncated)/);
      assert(!reader.finished);
    } finally { reader.close(); }
  }
});

void test("weighted summaries match an independent batch calculation and retain a brief peak", () => {
  const size = 100001, patches = [...patchSchedule(size)], summary = new TrackSummary();
  const values = patches.map((_, i) => i === patches.length - 1 ? 1 : 0.1);
  patches.forEach((patch, i) => summary.add(patch, scores(values[i])));
  const result = summary.finish(size);
  const weights = patches.map(patch => patch.weight_end_sample - patch.weight_start_sample);
  const mean = values.reduce((sum, value, i) => sum + weights[i] * value, 0) / size;
  const deviation = Math.sqrt(values.reduce((sum, value, i) => sum + weights[i] * (value - mean) ** 2, 0) / size);
  assert(Math.abs(result.mood.mean[0] - mean) < 1e-15);
  assert(Math.abs(result.instrument.stddev[0] - deviation) < 1e-15);
  assert.equal(result.mood.max[0], 1);
  assert(mean < 0.5, "A brief ending must not become the entire recording");
  assert(Math.abs(mean - values.reduce((sum, value) => sum + value, 0) / values.length) > 0.01);
});

void test("partial, overlapping, gapped and invalid summaries are rejected", () => {
  const summary = new TrackSummary(), first = { weight_start_sample: 0, weight_end_sample: 100 };
  assert.throws(() => summary.finish(100), /Incomplete/);
  for (const bad of [NaN, -0.1, 1.1]) assert.throws(() => summary.add(first, scores(bad)), /Invalid/);
  summary.add(first, scores(0.5));
  for (const begin of [99, 101, NaN]) {
    assert.throws(() => summary.add({ weight_start_sample: begin, weight_end_sample: 200 }, scores(0.2)), /Weights/);
  }
  assert.throws(() => summary.finish(200), /Incomplete/);
  assert.deepEqual(summary.finish(100).mood.stddev, Array(56).fill(0));
});

void test("streamed model patches equal direct independently cut reference patches", async t => {
  const root = temporary(t), samples = Float32Array.from({ length: 100001 }, (_, i) => Math.sin(i / 13));
  samples.fill(0, 20000, 40000);
  const { file } = pcm(root, samples), selected = [...patchSchedule(samples.length)];
  const fake: ReferenceRuntime = { ...runtime, infer: async (mel: Float32Array) => {
    const next = selected.shift(); assert.ok(next);
    const patch = patchSamples(samples, next.frame_start);
    const expected: number[] = [];
    for (let frame = 0; frame < 128; frame++) expected.push(...runtime.transform(Float32Array.from(patch.samples.slice(frame * 256, frame * 256 + 512))));
    assert.deepEqual(Array.from(mel), expected);
    return scores(0.5);
  } };
  const track = await streamTrack(file, fake);
  assert.equal(selected.length, 0);
  assert.equal(track.covered_samples, samples.length);
  assert.deepEqual(track.summary.mood.mean, Array(56).fill(0.5));
});

void test("cancellation before inference and after a result never emits a partial track summary", async t => {
  const root = temporary(t), { file } = pcm(root, new Float32Array(40000));
  for (const when of ["before", "inference", "after"]) {
    const controller = new AbortController();
    if (when === "before") controller.abort();
    let emitted = 0, calls = 0;
    await assert.rejects(streamTrack(file, { ...runtime, infer: async () => {
      calls++; if (when === "inference") controller.abort(); return scores(0.5);
    } }, { signal: controller.signal, onPatch: () => { emitted++; controller.abort(); } }), { name: "AbortError" });
    assert.equal(calls, when === "before" ? 0 : 1);
    assert.equal(emitted, when === "after" ? 1 : 0);
  }
});

void test("run completion follows cleanup and every track; output cannot overwrite an existing file", async t => {
  const root = temporary(t), { file } = pcm(root, new Float32Array(40000));
  const inputs = path.join(root, "paths.json"), output = path.join(root, "report.jsonl");
  fs.writeFileSync(inputs, JSON.stringify([file]));
  const result = await generateStreamReference({ inputs, output }, async (_, consume) => {
    const result = await consume(runtime);
    assert(!fs.readFileSync(output, "utf8").includes('"record_type":"complete"'));
    return result;
  });
  const before = fs.readFileSync(output), rows = before.toString().trim().split("\n").map((row) => JSON.parse(row));
  assert.equal(result.status, "complete"); assert.equal(result.tracks, 1);
  assert.equal(rows[0].schema_version, "effnet-stream-reference/v2");
  assert.equal(rows.at(-2).record_type, "track"); assert.equal(rows.at(-1).record_type, "complete");
  assert.equal(rows.filter(row => row.record_type === "patch").length, result.patches);
  await assert.rejects(generateStreamReference({ inputs, output }), { code: "EEXIST" });
  assert.deepEqual(fs.readFileSync(output), before);
});

void test("cancelled and failed runs close outputs and cannot write a success marker", async t => {
  const root = temporary(t), { file } = pcm(root, new Float32Array(40000)), inputs = path.join(root, "paths.json");
  fs.writeFileSync(inputs, JSON.stringify([file]));
  for (const when of ["cancel", "inference", "cleanup"]) {
    const output = path.join(root, when + ".jsonl"), controller = new AbortController();
    let released = false;
    const task = generateStreamReference({ inputs, output, signal: controller.signal }, async (_, consume) => {
      let result: Awaited<ReturnType<typeof consume>> | undefined;
      let inferenceError: unknown;
      try {
        result = await consume({ ...runtime, infer: async () => {
          if (when === "cancel") controller.abort();
          if (when === "inference") throw Error("failed graph");
          return scores(0.5);
        } });
      } catch (error: unknown) { inferenceError = error; }
      released = true;
      if (when === "cleanup") throw Error("failed cleanup");
      if (inferenceError !== undefined) throw inferenceError instanceof Error ? inferenceError : new Error("Inference failed", { cause: inferenceError });
      assert(result !== undefined, "Runtime consumer did not return a result");
      return result;
    });
    if (when === "cancel") {
      assert.deepEqual(await task, { status: "cancelled", tracks: 0, patches: 0 });
    } else await assert.rejects(task, /failed/);
    assert(released);
    const rows = fs.readFileSync(output, "utf8").trim().split("\n").map((row) => JSON.parse(row));
    assert(!rows.some(row => row.record_type === "complete"));
    if (when === "cancel") assert(!rows.some(row => row.record_type === "track"));
  }
});

void test("custom runtimes may report cancellation with a structurally named error", async t => {
  const root = temporary(t), { file } = pcm(root, new Float32Array(40000));
  const inputs = path.join(root, "paths.json"), output = path.join(root, "cancelled.jsonl");
  fs.writeFileSync(inputs, JSON.stringify([file]));
  const result = await generateStreamReference({ inputs, output }, async () => {
    const error = new Error("cancelled");
    error.name = "AbortError";
    throw error;
  });
  assert.deepEqual(result, { status: "cancelled", tracks: 0, patches: 0 });
  const rows = fs.readFileSync(output, "utf8").trim().split("\n").map((row) => JSON.parse(row));
  assert.deepEqual(rows, [{ record_type: "cancelled", ...result }]);
});
