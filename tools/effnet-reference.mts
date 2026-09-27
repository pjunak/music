// Offline qualification only: explicit local artifacts, no downloads or application imports.
import assert from "node:assert/strict";
import { createHash } from "node:crypto";
import fs from "node:fs";
import { createRequire } from "node:module";
import path from "node:path";
import { parseArgs } from "node:util";
import { pathToFileURL } from "node:url";
import { loadEssentiaReference } from "./musicnn-reference.mts";

export const SAMPLE_RATE = 16_000, FRAME_SIZE = 512, HOP = 256, PATCH_FRAMES = 128, PATCH_HOP = 62;
export const MAX_SAMPLES = SAMPLE_RATE * 15 * 60;
type ModelRole = "encoder" | "mood" | "instrument";
type ModelArtifact = readonly [role: ModelRole, file: string, sha256: string];
type MetadataArtifact = readonly [file: string, sha256: string, name: string, labels: number];
type RuntimeArtifact = readonly [file: string, sha256: string];
export interface ReferenceOptions {
  inputs: string;
  essentia?: string;
  ort?: string;
  models?: string;
  output: string;
  signal?: AbortSignal;
}
export interface ReferenceScores { embeddings: number[]; mood: number[]; instrument: number[] }
export interface ReferenceRuntime {
  header: Record<string, unknown>;
  transform(samples: Float32Array): Float32Array;
  infer(mel: Float32Array, signal?: AbortSignal): Promise<ReferenceScores>;
}
interface MetadataOutput { output_purpose?: string; shape?: unknown[] }
interface ModelMetadata {
  name?: string;
  version?: string;
  inference?: { sample_rate?: number; embedding_model?: { model_name?: string } };
  classes?: unknown[];
  schema?: { outputs?: MetadataOutput[] };
}
interface OrtTensor { dims: readonly number[]; data: ArrayLike<number>; dispose(): void }
interface OrtSession {
  inputNames: string[];
  outputNames: string[];
  run(inputs: Record<string, OrtTensor>): Promise<Record<string, OrtTensor>>;
  release(): Promise<void> | void;
}
interface OrtRuntime {
  env: { wasm: { numThreads: number; proxy: boolean } };
  Tensor: new (type: "float32", data: Float32Array, dimensions: number[]) => OrtTensor;
  InferenceSession: { create(bytes: Buffer, options: Record<string, unknown>): Promise<OrtSession> };
}

const MODELS: ModelArtifact[] = [
  ["encoder", "discogs-effnet-bsdynamic-1.onnx", "a280825b334797cf677939db8cd5762c0392aedd0ca6415dbc1cd083f045e43c"],
  ["mood", "mtg_jamendo_moodtheme-discogs-effnet-1.onnx", "7d6270acaa5f4bba4b115a0d6849aca05ed6bd153dcb6d9da4f6ab9f99ef10ff"],
  ["instrument", "mtg_jamendo_instrument-discogs-effnet-1.onnx", "9ae2d9e763d66bd8eed654d1ac3aa171e6539cb8a0e11f3dcd53df1428980802"],
];
const MODEL_METADATA: Record<ModelRole, MetadataArtifact> = {
  encoder: ["discogs-effnet-bsdynamic-1.json", "a2e85b2e7372d5f8e0f35bdd6aeae1139f101087d183d0b2fb60b0ea0f01a0ff", "EffnetDiscogs", 400],
  mood: ["mtg_jamendo_moodtheme-discogs-effnet-1.json", "d62cd90263e4d613fa7fcce7a831e339450394794af63685f96e065c1a896ab0", "mtg_jamendo_moodtheme", 56],
  instrument: ["mtg_jamendo_instrument-discogs-effnet-1.json", "7d02204c6451b5615e2968ec6364bbae3b915c886e608f05f00d3a38dc5177c4", "mtg_jamendo_instrument", 40],
};
const ORT_ARTIFACTS: RuntimeArtifact[] = [
  ["dist/ort-wasm-simd-threaded.mjs", "e13f7f94fc51b4ca72b12faeb1ee95f4ace6dfbc8939bc718aabdc0a27c4299b"],
  ["dist/ort.node.min.js", "f2ffa91920b249103bbfeb58a1a9b68bf92e9e9018bd164dc16da52bd14ee305"],
  ["dist/ort-wasm-simd-threaded.wasm", "3398c10d07d229bd91b364548e130e0e51a8e5704b88c7c083ebbeb78842dee2"],
];
const sha256 = (bytes: Buffer): string => createHash("sha256").update(bytes).digest("hex");

export function readBounded(file: string, limit: number): Buffer {
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

export function decodePcm(bytes: Buffer): Float32Array {
  assert(bytes.length > 0 && bytes.length % 4 === 0 && bytes.length <= MAX_SAMPLES * 4,
    "Expected nonempty float32-le mono PCM at 16 kHz, at most 15 minutes");
  const samples = new Float32Array(bytes.length / 4);
  for (let i = 0; i < samples.length; i++) {
    samples[i] = bytes.readFloatLE(i * 4);
    assert(Number.isFinite(samples[i]), "Non-finite PCM");
  }
  return samples;
}

export function frameCount(sampleCount: number): number {
  assert(Number.isSafeInteger(sampleCount) && sampleCount > 0 && sampleCount <= MAX_SAMPLES, "Invalid sample count");
  return Math.ceil(sampleCount / HOP) + 1;
}

export function selectPatches(sampleCount: number): Array<{ position: string; frame_start: number }> {
  const last = frameCount(sampleCount) - PATCH_FRAMES;
  assert(last >= 0, "Audio is too short for one complete patch");
  const seen = new Set<number>();
  const candidates: Array<[string, number]> = [["beginning", 0], ["middle", Math.floor(last / 2 / PATCH_HOP) * PATCH_HOP], ["ending", last]];
  return candidates
    .filter(([, start]) => {
      if (seen.has(start)) return false;
      seen.add(start);
      return true;
    })
    .map(([position, frame_start]) => ({ position, frame_start }));
}

export function patchSamples(samples: Float32Array, startFrame: number) {
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

export function readInputs(file: string): string[] {
  const inputs = JSON.parse(readBounded(file, 1024 * 1024).toString("utf8"));
  assert(Array.isArray(inputs) && inputs.length > 0 && inputs.length <= 32 &&
    inputs.every(item => typeof item === "string" && path.isAbsolute(item)) &&
    new Set(inputs).size === inputs.length, "Expected 1-32 unique absolute PCM paths");
  return inputs as string[];
}

function outputValues(tensor: OrtTensor, dimensions: number[], unitInterval = false): number[] {
  assert.deepEqual(tensor?.dims, dimensions, "Unexpected model output dimensions");
  const values = Array.from(tensor.data);
  assert.equal(values.length, dimensions.reduce((a, b) => a * b, 1));
  assert(values.every(value => Number.isFinite(value) && (!unitInterval || (value >= 0 && value <= 1))),
    "Invalid model output");
  return values;
}

// Catalog tensor names describe TensorFlow exports, not the ONNX session interface.
const isModelRole = (role: string): role is ModelRole => role === "encoder" || role === "mood" || role === "instrument";
const hasControlCharacter = (value: string): boolean => {
  for (let index = 0; index < value.length; index++) {
    const code = value.charCodeAt(index);
    if (code <= 0x1f || code === 0x7f) return true;
  }
  return false;
};

export function projectModelMetadata(role: string, metadata: ModelMetadata) {
  assert(isModelRole(role), "Unknown model role");
  const [, , name, labels] = MODEL_METADATA[role];
  assert.equal(metadata.name, name, "Wrong model metadata");
  assert.equal(metadata.version, "1", "Metadata version changed");
  assert.equal(metadata.inference?.sample_rate, SAMPLE_RATE, "Unexpected input sample rate");
  assert(Array.isArray(metadata.classes) && metadata.classes.length === labels &&
    metadata.classes.every((label): label is string => typeof label === "string" && label.length > 0 && label.length <= 128 &&
      label.trim() === label && !hasControlCharacter(label)) &&
    new Set(metadata.classes).size === labels, "Invalid label order");
  const purpose = role === "encoder" ? "embeddings" : "predictions";
  const outputs = metadata.schema?.outputs?.filter(output => output.output_purpose === purpose);
  const width = role === "encoder" ? 1280 : labels;
  assert(outputs?.length === 1 && Array.isArray(outputs[0].shape) &&
    outputs[0].shape.at(-1) === width, "Unexpected documented output");
  if (role === "encoder") return { name, version: metadata.version, embedding_dimensions: width };
  const embeddingModel = metadata.inference?.embedding_model?.model_name;
  assert.equal(embeddingModel, "discogs-effnet-bs64-1", "Unqualified head/encoder pairing");
  // Preserve exact spelling and order; no sorting, aliases or vocabulary mapping.
  return { name, version: metadata.version, labels: [...metadata.classes], documented_encoder: embeddingModel };
}

export function loadModelMetadata(directory: string) {
  return Object.fromEntries(MODELS.map(([role]) => {
    const [file, expected] = MODEL_METADATA[role];
    const bytes = readBounded(path.join(directory, file), 1024 * 1024);
    assert.equal(sha256(bytes), expected, "Unverified model metadata");
    const parsed = JSON.parse(bytes.toString("utf8")) as ModelMetadata;
    return [role, { file, sha256: expected, ...projectModelMetadata(role, parsed) }];
  }));
}

export function checkCancelled(signal?: AbortSignal): void {
  if (signal?.aborted) throw new DOMException("Reference cancelled", "AbortError");
}

export async function withReferenceRuntime<T>(options: ReferenceOptions, consume: (runtime: ReferenceRuntime) => T | Promise<T>): Promise<T> {
  checkCancelled(options.signal);
  assert(options.models && options.ort && options.essentia, "Reference runtime paths are required");
  const metadata = loadModelMetadata(options.models);
  const ortRoot = path.resolve(options.ort);
  const manifest: { name?: string; version?: string } = JSON.parse(readBounded(path.join(ortRoot, "package.json"), 1024 * 1024).toString("utf8"));
  assert.equal(manifest.name, "onnxruntime-web");
  assert.equal(manifest.version, "1.30.0", "Reference runtime changed; review it explicitly");
  for (const [file, expected] of ORT_ARTIFACTS) {
    assert.equal(sha256(readBounded(path.join(ortRoot, file), 64 * 1024 * 1024)), expected, "Unverified runtime artifact");
  }
  const require = createRequire(import.meta.url);
  const ort: OrtRuntime = require(path.join(ortRoot, "dist", "ort.node.min.js"));
  ort.env.wasm.numThreads = 1;
  ort.env.wasm.proxy = false;
  const { essentia, artifacts: frontendArtifacts } = loadEssentiaReference(options.essentia);
  const sessions: Partial<Record<ModelRole, OrtSession>> = {};
  const artifacts: Partial<Record<ModelRole, Record<string, unknown>>> = {};
  const release = async (): Promise<void> => {
    const releaseEssentia = (): void => {
      let shutdownError: unknown;
      try { essentia.shutdown(); } catch (error: unknown) { shutdownError = error; }
      essentia.delete();
      if (shutdownError !== undefined) throw shutdownError instanceof Error ? shutdownError : new Error("Essentia shutdown failed", { cause: shutdownError });
    };
    // Attempt every release even if one runtime cleanup fails.
    const released = await Promise.allSettled([
      Promise.resolve().then(releaseEssentia),
      ...Object.values(sessions).map((session) => Promise.resolve().then(() => session.release())),
    ]);
    const failed = released.find((result) => result.status === "rejected");
    if (failed) throw failed.reason;
  };
  let consumed: T;
  try {
    assert.equal(essentia.version, "2.1-beta6-dev");
    for (const [name, file, expected] of MODELS) {
      checkCancelled(options.signal);
      const bytes = readBounded(path.join(options.models, file), 64 * 1024 * 1024);
      assert.equal(sha256(bytes), expected, "Unverified model artifact");
      sessions[name] = await ort.InferenceSession.create(bytes, { executionProviders: ["wasm"], graphOptimizationLevel: "all" });
      assert.equal(sessions[name].inputNames.length, 1);
      artifacts[name] = { file, sha256: expected, metadata: metadata[name],
        inputs: sessions[name].inputNames, outputs: sessions[name].outputNames };
    }
    checkCancelled(options.signal);
    const header = {
      runtime: "onnxruntime-web/1.30.0; wasm, single thread",
      frontend: "essentia.js/0.1.3; TensorflowInputMusiCNN", frontend_artifacts: frontendArtifacts,
      runtime_artifacts: ORT_ARTIFACTS, artifacts,
      score_semantics: "Uncalibrated head scores in the exact metadata label order; not listening judgments",
      framing: { sample_rate: SAMPLE_RATE, frame_size: FRAME_SIZE, hop: HOP, patch_frames: PATCH_FRAMES,
        patch_hop: PATCH_HOP, centered: true, silent_frames: "keep", final_patch: "anchor_at_last_frame" },
    };
    const transform = (samples: Float32Array): Float32Array => {
      const frame = essentia.arrayToVector(samples);
      let result;
      try {
        result = essentia.TensorflowInputMusiCNN(frame);
        const bands = Float32Array.from(essentia.vectorToArray(result.bands));
        assert(bands.length === 96 && bands.every(Number.isFinite), "Invalid reference features");
        return bands;
      } finally { result?.bands.delete(); frame.delete(); }
    };
    const session = (role: ModelRole): OrtSession => {
      const selected = sessions[role];
      assert(selected, `Missing ${role} session`);
      return selected;
    };
    const infer = async (mel: Float32Array, signal?: AbortSignal): Promise<ReferenceScores> => {
      checkCancelled(signal);
      const input = new ort.Tensor("float32", mel, [1, PATCH_FRAMES, 96]);
      let encoded, mood, instrument;
      try {
        const encoder = session("encoder"), moodHead = session("mood"), instrumentHead = session("instrument");
        encoded = await encoder.run({ [encoder.inputNames[0]]: input });
        checkCancelled(signal);
        const embeddings = outputValues(encoded.embeddings, [1, 1280]);
        mood = await moodHead.run({ [moodHead.inputNames[0]]: encoded.embeddings });
        checkCancelled(signal);
        instrument = await instrumentHead.run({ [instrumentHead.inputNames[0]]: encoded.embeddings });
        checkCancelled(signal);
        return { embeddings, mood: outputValues(mood.activations, [1, 56], true),
          instrument: outputValues(instrument.activations, [1, 40], true) };
      } finally {
        for (const tensor of new Set([input, ...Object.values(encoded ?? {}),
          ...Object.values(mood ?? {}), ...Object.values(instrument ?? {})])) tensor.dispose();
      }
    };
    consumed = await consume({ header, transform, infer });
  } catch (error: unknown) {
    await release();
    throw error;
  }
  await release();
  return consumed;
}

export async function generateReference(options: ReferenceOptions): Promise<{ tracks: number; patches: number }> {
  const inputs = readInputs(options.inputs);
  // Reserve before loading the reference runtime or processing private recordings.
  const output = fs.openSync(options.output, "wx");
  try {
    return await withReferenceRuntime(options, async runtime => {
      const write = (record: Record<string, unknown>): void => fs.writeFileSync(output, JSON.stringify(record) + "\n");
      write({
        record_type: "header", schema_version: "effnet-patch-reference/v2", ...runtime.header, tracks: inputs.length,
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

export async function main(args: string[]): Promise<{ tracks: number; patches: number }> {
  const { values } = parseArgs({
    args, options: Object.fromEntries(["inputs", "essentia", "ort", "models", "output"].map(key => [key, { type: "string" }])),
  });
  assert(["inputs", "essentia", "ort", "models", "output"].every(key => values[key]),
    "Usage: --inputs PCM_PATHS_JSON --essentia PACKAGE_DIRECTORY --ort PACKAGE_DIRECTORY --models DIRECTORY --output NEW_JSONL");
  const required = (key: string): string => { const value = values[key]; assert(value); return value; };
  return generateReference({ inputs: required("inputs"), essentia: required("essentia"), ort: required("ort"),
    models: required("models"), output: required("output") });
}

if (process.argv[1] && import.meta.url === pathToFileURL(process.argv[1]).href) {
  main(process.argv.slice(2)).then(result => console.log(JSON.stringify(result))).catch(() => {
    // Upstream exceptions may contain private paths or a complete embedded WASM source.
    console.error("EffNet reference failed. Check explicit arguments, pinned artifacts and bounded finite PCM input. Incomplete output is not a passing reference.");
    process.exitCode = 1;
  });
}
