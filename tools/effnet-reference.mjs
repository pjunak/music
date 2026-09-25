// Offline qualification only: explicit local artifacts, no downloads or application imports.
import assert from "node:assert/strict";
import { createHash } from "node:crypto";
import fs from "node:fs";
import { createRequire } from "node:module";
import path from "node:path";
import { parseArgs } from "node:util";
import { pathToFileURL } from "node:url";
import { loadEssentiaReference } from "./musicnn-reference.mjs";

export const SAMPLE_RATE = 16_000, FRAME_SIZE = 512, HOP = 256, PATCH_FRAMES = 128, PATCH_HOP = 62;
export const MAX_SAMPLES = SAMPLE_RATE * 15 * 60;
const MODELS = [
  ["encoder", "discogs-effnet-bsdynamic-1.onnx", "a280825b334797cf677939db8cd5762c0392aedd0ca6415dbc1cd083f045e43c"],
  ["mood", "mtg_jamendo_moodtheme-discogs-effnet-1.onnx", "7d6270acaa5f4bba4b115a0d6849aca05ed6bd153dcb6d9da4f6ab9f99ef10ff"],
  ["instrument", "mtg_jamendo_instrument-discogs-effnet-1.onnx", "9ae2d9e763d66bd8eed654d1ac3aa171e6539cb8a0e11f3dcd53df1428980802"],
];
const ORT_ARTIFACTS = [
  ["dist/ort-wasm-simd-threaded.mjs", "e13f7f94fc51b4ca72b12faeb1ee95f4ace6dfbc8939bc718aabdc0a27c4299b"],
  ["dist/ort.node.min.js", "f2ffa91920b249103bbfeb58a1a9b68bf92e9e9018bd164dc16da52bd14ee305"],
  ["dist/ort-wasm-simd-threaded.wasm", "3398c10d07d229bd91b364548e130e0e51a8e5704b88c7c083ebbeb78842dee2"],
];
const sha256 = bytes => createHash("sha256").update(bytes).digest("hex");

export function readBounded(file, limit) {
  const descriptor = fs.openSync(file, "r");
  try {
    const stat = fs.fstatSync(descriptor);
    assert(stat.isFile() && stat.size <= limit, "Input must be a bounded regular file");
    // The additional byte detects growth after the size check without an unbounded read.
    const bytes = Buffer.alloc(stat.size + 1);
    let used = 0, read;
    do {
      read = fs.readSync(descriptor, bytes, used, bytes.length - used, null);
      used += read;
    } while (read && used < bytes.length);
    assert.equal(used, stat.size, "Input changed while reading");
    return bytes.subarray(0, used);
  } finally { fs.closeSync(descriptor); }
}

export function decodePcm(bytes) {
  assert(bytes.length > 0 && bytes.length % 4 === 0 && bytes.length <= MAX_SAMPLES * 4,
    "Expected nonempty float32-le mono PCM at 16 kHz, at most 15 minutes");
  const samples = new Float32Array(bytes.length / 4);
  for (let i = 0; i < samples.length; i++) {
    samples[i] = bytes.readFloatLE(i * 4);
    assert(Number.isFinite(samples[i]), "Non-finite PCM");
  }
  return samples;
}

export function frameCount(sampleCount) {
  assert(Number.isSafeInteger(sampleCount) && sampleCount > 0 && sampleCount <= MAX_SAMPLES, "Invalid sample count");
  return Math.ceil(sampleCount / HOP) + 1;
}

export function selectPatches(sampleCount) {
  const last = frameCount(sampleCount) - PATCH_FRAMES;
  assert(last >= 0, "Audio is too short for one complete patch");
  const seen = new Set();
  return [["beginning", 0], ["middle", Math.floor(last / 2 / PATCH_HOP) * PATCH_HOP], ["ending", last]]
    .filter(([, start]) => {
      if (seen.has(start)) return false;
      seen.add(start);
      return true;
    })
    .map(([position, frame_start]) => ({ position, frame_start }));
}

export function patchSamples(samples, startFrame) {
  assert(Number.isSafeInteger(startFrame) && startFrame >= 0 &&
    startFrame + PATCH_FRAMES <= frameCount(samples.length), "Patch is outside the centered timeline");
  const begin = startFrame * HOP - FRAME_SIZE / 2;
  // FrameGenerator in Essentia.js 0.1.3 drops silent frames. Preserve absolute positions instead.
  const support = Float32Array.from({ length: (PATCH_FRAMES - 1) * HOP + FRAME_SIZE },
    (_, i) => samples[begin + i] ?? 0);
  assert(support.every(Number.isFinite), "Non-finite patch");
  return {
    sample_offset: 0, samples: Array.from(support),
    valid_start_sample: Math.max(0, begin),
    valid_end_sample: Math.min(samples.length, begin + support.length),
  };
}

export function readInputs(file) {
  const inputs = JSON.parse(readBounded(file, 1024 * 1024).toString("utf8"));
  assert(Array.isArray(inputs) && inputs.length > 0 && inputs.length <= 32 &&
    inputs.every(item => typeof item === "string" && path.isAbsolute(item)) &&
    new Set(inputs).size === inputs.length, "Expected 1-32 unique absolute PCM paths");
  return inputs;
}

function outputValues(tensor, dimensions, unitInterval = false) {
  assert.deepEqual(tensor?.dims, dimensions, "Unexpected model output dimensions");
  const values = Array.from(tensor.data);
  assert.equal(values.length, dimensions.reduce((a, b) => a * b, 1));
  assert(values.every(value => Number.isFinite(value) && (!unitInterval || (value >= 0 && value <= 1))),
    "Invalid model output");
  return values;
}

export function checkCancelled(signal) {
  if (signal?.aborted) throw new DOMException("Reference cancelled", "AbortError");
}

export async function withReferenceRuntime(options, consume) {
  checkCancelled(options.signal);
  const ortRoot = path.resolve(options.ort);
  const manifest = JSON.parse(readBounded(path.join(ortRoot, "package.json"), 1024 * 1024));
  assert.equal(manifest.name, "onnxruntime-web");
  assert.equal(manifest.version, "1.30.0", "Reference runtime changed; review it explicitly");
  for (const [file, expected] of ORT_ARTIFACTS) {
    assert.equal(sha256(readBounded(path.join(ortRoot, file), 64 * 1024 * 1024)), expected, "Unverified runtime artifact");
  }
  const require = createRequire(import.meta.url);
  const ort = require(path.join(ortRoot, "dist", "ort.node.min.js"));
  ort.env.wasm.numThreads = 1;
  ort.env.wasm.proxy = false;
  const { essentia, artifacts: frontendArtifacts } = loadEssentiaReference(options.essentia);
  const sessions = {}, artifacts = {};
  try {
    assert.equal(essentia.version, "2.1-beta6-dev");
    for (const [name, file, expected] of MODELS) {
      checkCancelled(options.signal);
      const bytes = readBounded(path.join(options.models, file), 64 * 1024 * 1024);
      assert.equal(sha256(bytes), expected, "Unverified model artifact");
      sessions[name] = await ort.InferenceSession.create(bytes, { executionProviders: ["wasm"], graphOptimizationLevel: "all" });
      assert.equal(sessions[name].inputNames.length, 1);
      artifacts[name] = { file, sha256: expected, inputs: sessions[name].inputNames, outputs: sessions[name].outputNames };
    }
    checkCancelled(options.signal);
    const header = {
      runtime: "onnxruntime-web/1.30.0; wasm, single thread",
      frontend: "essentia.js/0.1.3; TensorflowInputMusiCNN", frontend_artifacts: frontendArtifacts,
      runtime_artifacts: ORT_ARTIFACTS, artifacts,
      framing: { sample_rate: SAMPLE_RATE, frame_size: FRAME_SIZE, hop: HOP, patch_frames: PATCH_FRAMES,
        patch_hop: PATCH_HOP, centered: true, silent_frames: "keep", final_patch: "anchor_at_last_frame" },
    };
    const transform = samples => {
      const frame = essentia.arrayToVector(samples);
      let result;
      try {
        result = essentia.TensorflowInputMusiCNN(frame);
        const bands = Float32Array.from(essentia.vectorToArray(result.bands));
        assert(bands.length === 96 && bands.every(Number.isFinite), "Invalid reference features");
        return bands;
      } finally { result?.bands.delete(); frame.delete(); }
    };
    const infer = async (mel, signal) => {
      checkCancelled(signal);
      const input = new ort.Tensor("float32", mel, [1, PATCH_FRAMES, 96]);
      let encoded, mood, instrument;
      try {
        encoded = await sessions.encoder.run({ [sessions.encoder.inputNames[0]]: input });
        checkCancelled(signal);
        const embeddings = outputValues(encoded.embeddings, [1, 1280]);
        mood = await sessions.mood.run({ [sessions.mood.inputNames[0]]: encoded.embeddings });
        checkCancelled(signal);
        instrument = await sessions.instrument.run({ [sessions.instrument.inputNames[0]]: encoded.embeddings });
        checkCancelled(signal);
        return { embeddings, mood: outputValues(mood.activations, [1, 56], true),
          instrument: outputValues(instrument.activations, [1, 40], true) };
      } finally {
        for (const tensor of new Set([input, ...Object.values(encoded ?? {}),
          ...Object.values(mood ?? {}), ...Object.values(instrument ?? {})])) tensor.dispose();
      }
    };
    return await consume({ header, transform, infer });
  } finally {
    // Attempt every release even if one runtime cleanup fails.
    const released = await Promise.allSettled([
      Promise.resolve().then(() => { try { essentia.shutdown(); } finally { essentia.delete(); } }),
      ...Object.values(sessions).map(session => Promise.resolve().then(() => session.release())),
    ]);
    const failed = released.find(result => result.status === "rejected");
    if (failed) throw failed.reason;
  }
}

export async function generateReference(options) {
  const inputs = readInputs(options.inputs);
  // Reserve before loading the reference runtime or processing private recordings.
  const output = fs.openSync(options.output, "wx");
  try {
    return await withReferenceRuntime(options, async runtime => {
      const write = record => fs.writeFileSync(output, JSON.stringify(record) + "\n");
      write({
        record_type: "header", schema_version: "effnet-patch-reference/v1", ...runtime.header, tracks: inputs.length,
        scope: "Selected patches from common decoded PCM; not decoder, TensorFlow-export or whole-track inference parity",
      });
      let count = 0;
      for (let index = 0; index < inputs.length; index++) {
        const bytes = readBounded(inputs[index], MAX_SAMPLES * 4);
        const samples = decodePcm(bytes), pcmSha256 = sha256(bytes);
        for (const selected of selectPatches(samples.length)) {
          const patch = patchSamples(samples, selected.frame_start), mel = [];
          for (let j = 0; j < PATCH_FRAMES; j++) {
            mel.push(...runtime.transform(Float32Array.from(patch.samples.slice(j * HOP, j * HOP + FRAME_SIZE))));
          }
          const scores = await runtime.infer(Float32Array.from(mel), options.signal);
          write({ record_type: "patch", index, ...selected, decoded_samples: samples.length,
            frame_count: frameCount(samples.length), pcm_sha256: pcmSha256, ...patch, mel, ...scores });
          count++;
        }
      }
      write({ record_type: "complete", tracks: inputs.length, patches: count });
      return { tracks: inputs.length, patches: count };
    });
  } finally { fs.closeSync(output); }
}

export async function main(args) {
  const { values } = parseArgs({
    args, options: Object.fromEntries(["inputs", "essentia", "ort", "models", "output"].map(key => [key, { type: "string" }])),
  });
  assert(["inputs", "essentia", "ort", "models", "output"].every(key => values[key]),
    "Usage: --inputs PCM_PATHS_JSON --essentia PACKAGE_DIRECTORY --ort PACKAGE_DIRECTORY --models DIRECTORY --output NEW_JSONL");
  return generateReference(values);
}

if (process.argv[1] && import.meta.url === pathToFileURL(process.argv[1]).href) {
  main(process.argv.slice(2)).then(result => console.log(JSON.stringify(result))).catch(() => {
    // Upstream exceptions may contain private paths or a complete embedded WASM source.
    console.error("EffNet reference failed. Check explicit arguments, pinned artifacts and bounded finite PCM input. Incomplete output is not a passing reference.");
    process.exitCode = 1;
  });
}
