import assert from "node:assert/strict";
import { test } from "node:test";
import { execFile } from "node:child_process";
import fs from "node:fs/promises";
import { tmpdir } from "node:os";
import path from "node:path";
import { promisify } from "node:util";
import { fileURLToPath } from "node:url";
import { decodePcm, frameCount, MAX_SAMPLES, patchSamples, readBounded, readInputs, selectPatches, projectModelMetadata, loadModelMetadata } from "./effnet-reference.mts";

void test("float32-le input retains quiet and signed values without normalization", () => {
  const expected = [0, -0, 0.25, -2, Math.fround(1e-20)], bytes = Buffer.alloc(expected.length * 4);
  expected.forEach((value, i) => bytes.writeFloatLE(value, i * 4));
  assert.deepEqual(Array.from(decodePcm(bytes)), expected);
});

void test("invalid, empty and non-finite PCM is rejected", () => {
  for (const size of [0, 1, 3, 5]) assert.throws(() => decodePcm(Buffer.alloc(size)), /float32-le/);
  for (const value of [NaN, Infinity, -Infinity]) {
    const bytes = Buffer.alloc(4); bytes.writeFloatLE(value);
    assert.throws(() => decodePcm(bytes), /Non-finite/);
  }
});

void test("centered frame counts include both boundaries without silence filtering", () => {
  for (const [length, count] of [[1, 2], [255, 2], [256, 2], [257, 3], [512, 3], [513, 4], [32768, 129]]) {
    assert.equal(frameCount(length), count);
  }
  for (const length of [0, -1, 1.5, NaN, Infinity, MAX_SAMPLES + 1]) assert.throws(() => frameCount(length), /sample count/);
  assert.equal(frameCount(MAX_SAMPLES), 56251);
});

void test("patch selection uses the 62-frame interior grid and always reaches the final frame", () => {
  for (const size of [32257, 32768, 33025, 100000, MAX_SAMPLES]) {
    const selected = selectPatches(size), count = frameCount(size), ending = selected.at(-1);
    assert.ok(selected[0]); assert.ok(ending);
    assert.equal(selected[0].frame_start, 0);
    assert.equal(ending.frame_start + 128, count);
    assert.equal(new Set(selected.map(item => item.frame_start)).size, selected.length);
    assert(selected.length >= 1 && selected.length <= 3);
    const middle = selected.find(item => item.position === "middle");
    if (middle) assert.equal(middle.frame_start % 62, 0);
  }
  assert.deepEqual(selectPatches(32257), [{ position: "beginning", frame_start: 0 }]);
  assert.throws(() => selectPatches(32256), /too short/);
});

void test("beginning padding never shifts the original samples", () => {
  const samples = Float32Array.from({ length: 40000 }, (_, i) => i + 1), patch = patchSamples(samples, 0);
  assert.equal(patch.sample_offset, 0);
  assert.equal(patch.samples.length, 33024);
  assert(patch.samples.slice(0, 256).every(value => value === 0));
  assert.equal(patch.samples[256], 1);
  assert.equal(patch.samples.at(-1), 32768);
  assert.equal(patch.valid_start_sample, 0);
  assert.equal(patch.valid_end_sample, 32768);
});

void test("silent beginnings, internal gaps and endings retain their exact timeline positions", () => {
  const samples = new Float32Array(100001);
  samples[2048] = 0.25; samples[31000] = -0.5; samples[66000] = 1;
  for (const selected of selectPatches(samples.length)) {
    const patch = patchSamples(samples, selected.frame_start);
    const start = selected.frame_start * 256 - 256;
    for (let frame = 0; frame < 128; frame++) {
      const actual = patch.samples.slice(frame * 256, frame * 256 + 512);
      const expected = Array.from({ length: 512 }, (_, i) => samples[start + frame * 256 + i] ?? 0);
      assert.deepEqual(actual, expected);
    }
  }
  const beginning = patchSamples(samples, 0);
  assert.equal(beginning.samples[2048 + 256], 0.25);
  assert(beginning.samples.slice(0, 2048 + 256).every(value => value === 0));
  assert.equal(patchSamples(new Float32Array(samples.length), 0).samples.filter(Boolean).length, 0);
});

void test("ending padding follows the last original sample and never repeats the tail", () => {
  const samples = new Float32Array(100001); samples[samples.length - 1] = 0.75;
  const ending = selectPatches(samples.length).at(-1); assert.ok(ending);
  const start = ending.frame_start;
  const patch = patchSamples(samples, start), last = samples.length - 1 - (start * 256 - 256);
  assert.equal(patch.samples[last], 0.75);
  assert(patch.samples.slice(last + 1).every(value => value === 0));
  assert.equal(patch.valid_end_sample, samples.length);
  assert.equal(patch.samples.filter(value => value === 0.75).length, 1);
  const overlapping = patch.samples.slice(256);
  assert.deepEqual(overlapping.slice(0, 512), Array.from(samples.slice(start * 256, start * 256 + 512)));
});

void test("invalid patch positions and non-finite support fail explicitly", () => {
  const samples = new Float32Array(40000);
  for (const start of [-1, 0.5, NaN, 31]) assert.throws(() => patchSamples(samples, start), /outside/);
  samples[1] = NaN;
  assert.throws(() => patchSamples(samples, 0), /Non-finite/);
});

void test("bounded readers reject oversized files and invalid private manifests", async () => {
  const directory = await fs.mkdtemp(path.join(tmpdir(), "music-reference-"));
  try {
    const file = path.join(directory, "paths.json");
    await fs.writeFile(file, "12345");
    assert.throws(() => readBounded(file, 4), /bounded/);
    assert.equal(readBounded(file, 5).toString(), "12345");
    for (const values of [[], ["relative.pcm"], [file, file], [5], Array(33).fill(file)]) {
      await fs.writeFile(file, JSON.stringify(values));
      assert.throws(() => readInputs(file), /1-32 unique/);
    }
    await fs.writeFile(file, JSON.stringify([path.join(directory, "input.pcm")]));
    assert.deepEqual(readInputs(file), [path.join(directory, "input.pcm")]);
  } finally { await fs.rm(directory, { recursive: true, force: true }); }
});

void test("failed CLI does not print private paths, tensors or a reference completion", async () => {
  const cli = fileURLToPath(new URL("./effnet-reference.mts", import.meta.url));
  try {
    await promisify(execFile)(process.execPath, [cli, "--inputs", path.join(tmpdir(), "private-recording-name.json"),
      "--essentia", "absent", "--ort", "absent", "--models", "absent", "--output", "absent"]);
    assert.fail("Expected the missing-input command to fail");
  } catch (error: unknown) {
    assert(error instanceof Error && "code" in error && "stdout" in error && "stderr" in error);
    const failure = error as Error & { code: number; stdout: string; stderr: string };
    assert.equal(failure.code, 1);
    assert.equal(failure.stdout, "");
    assert.match(failure.stderr, /EffNet reference failed/);
    assert.doesNotMatch(failure.stderr, /private-recording-name|stack|at main/);
  }
});

interface MutableMetadata {
  name: string;
  version: string;
  classes: Array<string | number>;
  inference: { sample_rate: number; embedding_model: { model_name: string } };
  schema: { outputs: Array<{ output_purpose: string; shape: Array<string | number> }> };
}

function headMetadata(role: "mood" | "instrument" = "mood"): MutableMetadata {
  const length = role === "mood" ? 56 : 40;
  return {
    name: role === "mood" ? "mtg_jamendo_moodtheme" : "mtg_jamendo_instrument", version: "1",
    classes: Array.from({ length }, (_, i) => "label-" + i),
    inference: { sample_rate: 16000, embedding_model: { model_name: "discogs-effnet-bs64-1" } },
    schema: { outputs: [{ output_purpose: "predictions", shape: [length] }] },
  };
}

void test("head metadata retains the exact label positions independently of its input object", () => {
  for (const role of ["mood", "instrument"] as const) {
    const metadata = headMetadata(role);
    metadata.classes[0] = "z-last-alphabetically"; metadata.classes[1] = "A-first-alphabetically";
    const result = projectModelMetadata(role, metadata);
    assert.deepEqual(result.labels, metadata.classes);
    assert.notEqual(result.labels, metadata.classes);
    metadata.classes.reverse();
    assert.equal(result.labels[0], "z-last-alphabetically");
    assert.equal(result.documented_encoder, "discogs-effnet-bs64-1");
  }
});

void test("label counts, duplicates and malformed names cannot define score meanings", () => {
  const variants: Array<(metadata: ReturnType<typeof headMetadata>) => void> = [
    metadata => { metadata.classes.pop(); }, metadata => { metadata.classes.push("extra"); }, metadata => { metadata.classes[0] = metadata.classes[1]; },
    metadata => { metadata.classes[0] = ""; }, metadata => { metadata.classes[0] = " padded "; },
    metadata => { metadata.classes[0] = 1; }, metadata => { metadata.classes[0] = "line\nbreak"; },
    metadata => { metadata.classes[0] = "x".repeat(129); },
  ];
  for (const mutate of variants) {
    const metadata = headMetadata(); mutate(metadata);
    assert.throws(() => projectModelMetadata("mood", metadata), /label order/);
  }
});

void test("wrong model, version, sample rate, dimensions or encoder pairing fail explicitly", () => {
  const variants: Array<(metadata: ReturnType<typeof headMetadata>) => void> = [
    metadata => { metadata.name = "other-head"; }, metadata => { metadata.version = "2"; },
    metadata => { metadata.inference.sample_rate = 44100; }, metadata => { metadata.schema.outputs[0].shape = [40]; },
    metadata => { metadata.schema.outputs[0].output_purpose = "embeddings"; },
    metadata => { metadata.schema.outputs.push(metadata.schema.outputs[0]); },
    metadata => { metadata.inference.embedding_model.model_name = "other-1280-encoder"; },
  ];
  for (const mutate of variants) {
    const metadata = headMetadata(); mutate(metadata);
    assert.throws(() => projectModelMetadata("mood", metadata));
  }
  assert.throws(() => projectModelMetadata("constructor", headMetadata()), /Unknown model role/);
});

void test("encoder metadata qualifies both its 400 style activations and 1280 embeddings", () => {
  const metadata = {
    name: "EffnetDiscogs", version: "1", classes: Array.from({ length: 400 }, (_, i) => "style-" + i),
    inference: { sample_rate: 16000 },
    schema: { outputs: [{ output_purpose: "embeddings", shape: ["n", 1280] }] },
  };
  assert.deepEqual(projectModelMetadata("encoder", metadata),
    { name: "EffnetDiscogs", version: "1", embedding_dimensions: 1280 });
});

void test("unverified metadata fails before it can be used as a label map", async () => {
  const directory = await fs.mkdtemp(path.join(tmpdir(), "music-model-metadata-"));
  try {
    const file = path.join(directory, "discogs-effnet-bsdynamic-1.json");
    await fs.writeFile(file, JSON.stringify({ classes: ["looks-plausible"] }));
    assert.throws(() => loadModelMetadata(directory), /Unverified model metadata/);
  } finally { await fs.rm(directory, { recursive: true, force: true }); }
});
