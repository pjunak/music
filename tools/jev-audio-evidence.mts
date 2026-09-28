// Offline learned-audio export for bounded Jev experiments. No application imports or network access.
import assert from "node:assert/strict";
import { createHash } from "node:crypto";
import fs from "node:fs";
import path from "node:path";
import readline from "node:readline";
import { parseArgs } from "node:util";
import { pathToFileURL } from "node:url";
import { readBounded, readInputs, SAMPLE_RATE, withReferenceRuntime } from "./effnet-reference.mts";
import type { ReferenceRuntime } from "./effnet-reference.mts";
import { streamTrack } from "./effnet-stream.mts";

export const SCHEMA_VERSION = "song-audio-predictions/v1";
export const SCORE_KIND = "uncalibrated_sigmoid";
export const TOP_RANK_LIMIT = 3;
const AGGREGATION = "nearest_patch_center_time_partition/v1";
const HEX_SHA256 = /^[0-9a-f]{64}$/;

export type SourceKind = "instrument" | "mood_theme" | "style";
const SOURCE_KINDS: SourceKind[] = ["instrument", "mood_theme", "style"];
type ScoreVectors = Record<SourceKind, number[]>;

interface ModelSpec {
  kind: SourceKind;
  modelId: string;
  modelFile: string;
  modelSha256: string;
  taxonomyFile: string;
  taxonomySha256: string;
  labelOrderSha256: string;
  labelCount: number;
  metadataName: string;
}

const MODEL_SPECS: readonly ModelSpec[] = [
  {
    kind: "instrument", modelId: "mtg_jamendo_instrument-discogs-effnet-1",
    modelFile: "mtg_jamendo_instrument-discogs-effnet-1.onnx",
    modelSha256: "9ae2d9e763d66bd8eed654d1ac3aa171e6539cb8a0e11f3dcd53df1428980802",
    taxonomyFile: "mtg_jamendo_instrument-discogs-effnet-1.json",
    taxonomySha256: "7d02204c6451b5615e2968ec6364bbae3b915c886e608f05f00d3a38dc5177c4",
    labelOrderSha256: "02636df9eb46c754c057a443f72890fdf431819e9efb198e8e368e846888699e",
    labelCount: 40, metadataName: "mtg_jamendo_instrument",
  },
  {
    kind: "mood_theme", modelId: "mtg_jamendo_moodtheme-discogs-effnet-1",
    modelFile: "mtg_jamendo_moodtheme-discogs-effnet-1.onnx",
    modelSha256: "7d6270acaa5f4bba4b115a0d6849aca05ed6bd153dcb6d9da4f6ab9f99ef10ff",
    taxonomyFile: "mtg_jamendo_moodtheme-discogs-effnet-1.json",
    taxonomySha256: "d62cd90263e4d613fa7fcce7a831e339450394794af63685f96e065c1a896ab0",
    labelOrderSha256: "b7e6628ca5ceb51ffe7ef5e17d6538c1ac2d77346e26685eee193c018c20811c",
    labelCount: 56, metadataName: "mtg_jamendo_moodtheme",
  },
  {
    kind: "style", modelId: "discogs-effnet-bsdynamic-1",
    modelFile: "discogs-effnet-bsdynamic-1.onnx",
    modelSha256: "a280825b334797cf677939db8cd5762c0392aedd0ca6415dbc1cd083f045e43c",
    taxonomyFile: "discogs-effnet-bsdynamic-1.json",
    taxonomySha256: "a2e85b2e7372d5f8e0f35bdd6aeae1139f101087d183d0b2fb60b0ea0f01a0ff",
    labelOrderSha256: "80974bd84b6bd87a455fa3dedaf2be34673d4c672d8b898558ad1713baf0a78e",
    labelCount: 400, metadataName: "EffnetDiscogs",
  },
];

export interface Taxonomy extends ModelSpec { labels: string[] }
export interface EvidencePatch {
  weight_start_sample: number;
  weight_end_sample: number;
  scores: ScoreVectors;
}
export interface AggregationInput {
  file_sha256: string;
  pcm_sha256: string;
  decoded_samples: number;
  covered_samples: number;
  taxonomies: Record<SourceKind, Taxonomy>;
  patches: EvidencePatch[];
}
export interface EvidenceLabel {
  label: string;
  mean_score: number;
  max_score: number;
  top_rank_fraction: number;
  opening_score: number;
  ending_score: number;
}
export interface EvidenceSource {
  kind: SourceKind;
  model_id: string;
  model_sha256: string;
  taxonomy_sha256: string;
  score_kind: typeof SCORE_KIND;
  duration_seconds: number;
  covered_seconds: number;
  top_rank_limit: typeof TOP_RANK_LIMIT;
  labels: EvidenceLabel[];
}
export interface EvidenceRecording {
  file_sha256: string;
  pcm_sha256: string;
  duration_seconds: number;
  covered_seconds: number;
  sources: EvidenceSource[];
  prediction_sha256: string;
}
export interface EvidenceArtifact {
  schema_version: typeof SCHEMA_VERSION;
  score_kind: typeof SCORE_KIND;
  recording_set_sha256: string;
  aggregation: {
    patch_schedule: typeof AGGREGATION;
    top_rank_limit: typeof TOP_RANK_LIMIT;
    opening_fraction: 0.1;
    ending_fraction: 0.1;
  };
  recordings: EvidenceRecording[];
}

interface Metadata {
  name?: unknown;
  version?: unknown;
  classes?: unknown;
  inference?: { sample_rate?: unknown; embedding_model?: { model_name?: unknown } };
  schema?: { outputs?: Array<{ output_purpose?: unknown; shape?: unknown[] }> };
}
interface SourceManifestEntry {
  file?: unknown;
  bytes?: unknown;
  sha256?: unknown;
  pcm_file?: unknown;
  pcm_sha256?: unknown;
}
export interface CorpusBinding { fileSha256: string; pcmSha256: string }
export interface PrepareCorpusManifestOptions {
  inputs: string;
  sourceManifest: string;
  framing: string;
  reference: string;
  output: string;
}
export interface ExportOptions {
  inputs: string;
  corpusManifest: string;
  essentia: string;
  ort: string;
  models: string;
  output: string;
}

const sha256 = (bytes: Buffer): string => createHash("sha256").update(bytes).digest("hex");

export function canonicalJson(value: unknown): string {
  if (Array.isArray(value)) return `[${value.map(canonicalJson).join(",")}]`;
  if (isPlainObject(value)) return `{${Object.keys(value).sort().map(key =>
    `${JSON.stringify(key)}:${canonicalJson(value[key])}`).join(",")}}`;
  const encoded = JSON.stringify(value);
  assert(encoded !== undefined, "Unsupported canonical JSON value");
  return encoded;
}

function isPlainObject(value: unknown): value is Record<string, unknown> {
  return value !== null && typeof value === "object" && !Array.isArray(value);
}

function validLabels(value: unknown, count: number): value is string[] {
  return Array.isArray(value) && value.length === count && new Set(value).size === count &&
    value.every(label => typeof label === "string" && label.length > 0 && label.length <= 128 && label.trim() === label &&
      !Array.from(label).some(character => character.charCodeAt(0) <= 0x1f || character.charCodeAt(0) === 0x7f));
}

export function loadTaxonomies(directory: string): Record<SourceKind, Taxonomy> {
  const entries = MODEL_SPECS.map(spec => {
    const modelBytes = readBounded(path.join(directory, spec.modelFile), 64 * 1024 * 1024);
    assert.equal(sha256(modelBytes), spec.modelSha256, `Unverified ${spec.kind} model`);
    const metadataBytes = readBounded(path.join(directory, spec.taxonomyFile), 1024 * 1024);
    assert.equal(sha256(metadataBytes), spec.taxonomySha256, `Unverified ${spec.kind} taxonomy`);
    const metadata = JSON.parse(metadataBytes.toString("utf8")) as Metadata;
    assert.equal(metadata.name, spec.metadataName, `Wrong ${spec.kind} taxonomy`);
    assert.equal(metadata.version, "1", `Wrong ${spec.kind} taxonomy version`);
    assert.equal(metadata.inference?.sample_rate, SAMPLE_RATE, `Wrong ${spec.kind} sample rate`);
    assert(validLabels(metadata.classes, spec.labelCount), `Invalid ${spec.kind} label order`);
    assert.equal(sha256(Buffer.from(canonicalJson(metadata.classes))), spec.labelOrderSha256,
      `Changed ${spec.kind} label order`);
    const outputs = metadata.schema?.outputs?.filter(output => output.output_purpose === "predictions");
    assert(outputs?.length === 1 && Array.isArray(outputs[0].shape) && outputs[0].shape.at(-1) === spec.labelCount,
      `Wrong ${spec.kind} output dimensions`);
    if (spec.kind !== "style") {
      assert.equal(metadata.inference?.embedding_model?.model_name, "discogs-effnet-bs64-1", "Unqualified head/encoder pairing");
    }
    return [spec.kind, { ...spec, labels: [...metadata.classes] }] as const;
  });
  return Object.fromEntries(entries) as Record<SourceKind, Taxonomy>;
}

function validateScores(scores: unknown, count: number, context: string): asserts scores is number[] {
  assert(Array.isArray(scores) && scores.length === count &&
    scores.every(value => typeof value === "number" && Number.isFinite(value) && value >= 0 && value <= 1),
  `${context} must contain ${count} bounded scores`);
}

function topRankIndexes(scores: number[]): Set<number> {
  const indexes = scores.map((_, index) => index);
  indexes.sort((left, right) => scores[right] - scores[left] || left - right);
  return new Set(indexes.slice(0, TOP_RANK_LIMIT));
}

export function aggregateRecording(input: AggregationInput): EvidenceRecording {
  assert(HEX_SHA256.test(input.file_sha256), "Invalid source file hash");
  assert(HEX_SHA256.test(input.pcm_sha256), "Invalid PCM hash");
  assert(Number.isSafeInteger(input.decoded_samples) && input.decoded_samples > 0, "Invalid duration");
  assert.equal(input.covered_samples, input.decoded_samples, "Incomplete audio coverage");
  assert(input.patches.length > 0, "Missing patch predictions");
  const duration = input.decoded_samples / SAMPLE_RATE;
  const openingEnd = input.decoded_samples / 10;
  const endingStart = input.decoded_samples - openingEnd;
  let expectedStart = 0;
  for (const patch of input.patches) {
    assert(Number.isSafeInteger(patch.weight_start_sample) && Number.isSafeInteger(patch.weight_end_sample) &&
      patch.weight_start_sample === expectedStart && patch.weight_end_sample > patch.weight_start_sample &&
      patch.weight_end_sample <= input.decoded_samples, "Patch weights must partition the recording exactly");
    for (const kind of SOURCE_KINDS) validateScores(patch.scores[kind], input.taxonomies[kind].labelCount, `${kind} patch`);
    expectedStart = patch.weight_end_sample;
  }
  assert.equal(expectedStart, input.decoded_samples, "Missing ending coverage");

  const sources = SOURCE_KINDS.map(kind => {
    const taxonomy = input.taxonomies[kind];
    assert.equal(taxonomy.labels.length, taxonomy.labelCount, `Wrong ${kind} label order`);
    const sums = new Float64Array(taxonomy.labelCount);
    const maxima = new Float64Array(taxonomy.labelCount);
    const ranks = new Float64Array(taxonomy.labelCount);
    const openings = new Float64Array(taxonomy.labelCount);
    const endings = new Float64Array(taxonomy.labelCount);
    for (const patch of input.patches) {
      const weight = patch.weight_end_sample - patch.weight_start_sample;
      const openingWeight = Math.max(0, Math.min(patch.weight_end_sample, openingEnd) - patch.weight_start_sample);
      const endingWeight = Math.max(0, patch.weight_end_sample - Math.max(patch.weight_start_sample, endingStart));
      const ranked = topRankIndexes(patch.scores[kind]);
      patch.scores[kind].forEach((score, index) => {
        sums[index] += score * weight;
        maxima[index] = Math.max(maxima[index], score);
        if (ranked.has(index)) ranks[index] += weight;
        openings[index] += score * openingWeight;
        endings[index] += score * endingWeight;
      });
    }
    return {
      kind, model_id: taxonomy.modelId, model_sha256: taxonomy.modelSha256,
      taxonomy_sha256: taxonomy.taxonomySha256, score_kind: SCORE_KIND,
      duration_seconds: duration, covered_seconds: input.covered_samples / SAMPLE_RATE,
      top_rank_limit: TOP_RANK_LIMIT,
      labels: taxonomy.labels.map((label, index) => ({
        label, mean_score: sums[index] / input.decoded_samples, max_score: maxima[index],
        top_rank_fraction: ranks[index] / input.decoded_samples,
        opening_score: openings[index] / openingEnd, ending_score: endings[index] / openingEnd,
      })),
    } satisfies EvidenceSource;
  });
  const prediction = { file_sha256: input.file_sha256, pcm_sha256: input.pcm_sha256, duration_seconds: duration,
    covered_seconds: input.covered_samples / SAMPLE_RATE, sources };
  return { ...prediction, prediction_sha256: sha256(Buffer.from(canonicalJson(prediction))) };
}

export function buildArtifact(recordings: EvidenceRecording[]): EvidenceArtifact {
  assert(recordings.length > 0 && recordings.length <= 32, "Expected 1-32 recordings");
  assert.equal(new Set(recordings.map(recording => recording.file_sha256)).size, recordings.length,
    "Duplicate source file hash");
  for (const recording of recordings) {
    const { prediction_sha256, ...prediction } = recording;
    assert.equal(prediction_sha256, sha256(Buffer.from(canonicalJson(prediction))), "Recording prediction hash mismatch");
  }
  return {
    schema_version: SCHEMA_VERSION, score_kind: SCORE_KIND,
    recording_set_sha256: sha256(Buffer.from(canonicalJson(recordings.map(recording => recording.prediction_sha256)))),
    aggregation: { patch_schedule: AGGREGATION, top_rank_limit: TOP_RANK_LIMIT,
      opening_fraction: 0.1, ending_fraction: 0.1 },
    recordings,
  };
}

function hashFile(file: string, expectedBytes: number): string {
  assert(Number.isSafeInteger(expectedBytes) && expectedBytes > 0, "Invalid source size");
  const stat = fs.statSync(file);
  assert(stat.isFile() && stat.size === expectedBytes, "Source file size changed");
  const hash = createHash("sha256"), descriptor = fs.openSync(file, "r"), bytes = Buffer.alloc(1024 * 1024);
  try {
    let total = 0, count;
    do {
      count = fs.readSync(descriptor, bytes, 0, bytes.length, null);
      if (count) { hash.update(bytes.subarray(0, count)); total += count; }
    } while (count);
    assert.equal(total, expectedBytes, "Source file changed while hashing");
    return hash.digest("hex");
  } finally { fs.closeSync(descriptor); }
}

async function readReferencePcmHashes(file: string, expectedCount: number): Promise<string[]> {
  const stat = fs.statSync(file);
  assert(stat.isFile() && stat.size > 0 && stat.size <= 256 * 1024 * 1024, "Invalid bounded reference");
  const lines = readline.createInterface({ input: fs.createReadStream(file, { encoding: "utf8" }), crlfDelay: Infinity });
  const hashes: string[] = [];
  let header = false, complete = false;
  for await (const line of lines) {
    assert(line.length > 0 && line.length <= 1024 * 1024, "Invalid bounded reference line");
    if (!line.includes('"record_type":"header"') && !line.includes('"record_type":"track"') &&
      !line.includes('"record_type":"complete"')) continue;
    const record: unknown = JSON.parse(line);
    assert(isPlainObject(record), "Invalid reference record");
    if (record.record_type === "header") {
      assert(!header && record.tracks === expectedCount, "Reference header mismatch");
      header = true;
    } else if (record.record_type === "track") {
      assert(record.index === hashes.length && typeof record.pcm_sha256 === "string" && HEX_SHA256.test(record.pcm_sha256),
        "Reference track order/hash mismatch");
      hashes.push(record.pcm_sha256);
    } else {
      assert(record.status === "complete" && record.tracks === expectedCount, "Incomplete reference");
      complete = true;
    }
  }
  assert(header && complete && hashes.length === expectedCount, "Incomplete reference PCM mapping");
  return hashes;
}

export async function prepareCorpusManifest(options: PrepareCorpusManifestOptions): Promise<{ recordings: number }> {
  const inputs = readInputs(options.inputs);
  const sources: unknown = JSON.parse(readBounded(options.sourceManifest, 1024 * 1024).toString("utf8"));
  const framing: unknown = JSON.parse(readBounded(options.framing, 1024 * 1024).toString("utf8"));
  assert(Array.isArray(sources) && sources.length === inputs.length, "Source manifest count mismatch");
  assert(Array.isArray(framing) && framing.length === inputs.length, "Framing manifest count mismatch");
  const referenceHashes = await readReferencePcmHashes(options.reference, inputs.length);
  const entries = sources.map((source: SourceManifestEntry, index) => {
    assert(isPlainObject(source) && typeof source.file === "string" && path.isAbsolute(source.file) &&
      typeof source.bytes === "number" && typeof source.sha256 === "string" && HEX_SHA256.test(source.sha256),
    "Invalid source manifest entry");
    assert.equal(hashFile(source.file, source.bytes), source.sha256, "Source hash mismatch");
    const observation = framing[index];
    assert(isPlainObject(observation) && observation.index === index &&
      typeof observation.pcm_sha256 === "string" && HEX_SHA256.test(observation.pcm_sha256),
    "Invalid framing mapping");
    assert.equal(path.basename(inputs[index]), `track-${index}.f32le`, "Unexpected preparation path mapping");
    assert.equal(observation.pcm_sha256, referenceHashes[index], "Preparation/reference PCM hash mismatch");
    assert.equal(hashFile(inputs[index], fs.statSync(inputs[index]).size), referenceHashes[index], "PCM hash mismatch");
    return { file: source.file, bytes: source.bytes, sha256: source.sha256,
      pcm_file: inputs[index], pcm_sha256: referenceHashes[index] };
  });
  fs.writeFileSync(options.output, JSON.stringify(entries) + "\n", { flag: "wx" });
  return { recordings: entries.length };
}

export function readCorpusManifest(file: string, inputs: string[]): CorpusBinding[] {
  const value = JSON.parse(readBounded(file, 1024 * 1024).toString("utf8"));
  assert(Array.isArray(value) && value.length === inputs.length, "Corpus manifest count mismatch");
  return value.map((item: SourceManifestEntry, index) => {
    assert(isPlainObject(item) && typeof item.file === "string" && path.isAbsolute(item.file) &&
      typeof item.bytes === "number" && typeof item.sha256 === "string" && HEX_SHA256.test(item.sha256) &&
      typeof item.pcm_file === "string" && path.isAbsolute(item.pcm_file) &&
      typeof item.pcm_sha256 === "string" && HEX_SHA256.test(item.pcm_sha256), "Invalid corpus manifest entry");
    assert.equal(path.resolve(item.pcm_file), path.resolve(inputs[index]), "PCM path/order mismatch");
    assert.equal(hashFile(item.file, item.bytes), item.sha256, "Source hash mismatch");
    assert.equal(hashFile(item.pcm_file, fs.statSync(item.pcm_file).size), item.pcm_sha256, "PCM hash mismatch");
    return { fileSha256: item.sha256, pcmSha256: item.pcm_sha256 };
  });
}

export function validateStreamedPcm(binding: CorpusBinding, observed: unknown): void {
  assert.equal(observed, binding.pcmSha256, "Streamed PCM hash mismatch");
}

async function exportTrack(file: string, binding: CorpusBinding, taxonomies: Record<SourceKind, Taxonomy>,
  runtime: ReferenceRuntime): Promise<{ recording: EvidenceRecording; patches: number }> {
  const patches: EvidencePatch[] = [];
  const track = await streamTrack(file, runtime, { onPatch: patch => {
    const scores: ScoreVectors = { instrument: patch.instrument, mood_theme: patch.mood, style: patch.style };
    for (const kind of SOURCE_KINDS) validateScores(scores[kind], taxonomies[kind].labelCount, `${kind} patch`);
    patches.push({ weight_start_sample: patch.weight_start_sample, weight_end_sample: patch.weight_end_sample, scores });
  } });
  validateStreamedPcm(binding, track.pcm_sha256);
  return { recording: aggregateRecording({ file_sha256: binding.fileSha256, pcm_sha256: binding.pcmSha256,
    decoded_samples: track.decoded_samples,
    covered_samples: track.covered_samples, taxonomies, patches }), patches: track.patches };
}

export async function generateEvidence(options: ExportOptions): Promise<{ recordings: number; patches: number }> {
  const inputs = readInputs(options.inputs);
  const bindings = readCorpusManifest(options.corpusManifest, inputs);
  const taxonomies = loadTaxonomies(options.models);
  const output = fs.openSync(options.output, "wx");
  try {
    const recordings: EvidenceRecording[] = [];
    let patches = 0;
    await withReferenceRuntime({ ...options, output: options.output }, async runtime => {
      for (let index = 0; index < inputs.length; index++) {
        const result = await exportTrack(inputs[index], bindings[index], taxonomies, runtime);
        recordings.push(result.recording);
        patches += result.patches;
      }
    });
    // Re-read and rehash both sides after inference so the exported binding cannot hide mid-run changes.
    assert.deepEqual(readCorpusManifest(options.corpusManifest, inputs), bindings, "Corpus changed during export");
    fs.writeFileSync(output, JSON.stringify(buildArtifact(recordings)) + "\n");
    return { recordings: recordings.length, patches };
  } finally { fs.closeSync(output); }
}

export async function main(args: string[]): Promise<{ recordings: number; patches: number }> {
  const { values } = parseArgs({ args, options: Object.fromEntries(
    ["inputs", "corpus-manifest", "essentia", "ort", "models", "output"].map(key => [key, { type: "string" }])) });
  assert(["inputs", "corpus-manifest", "essentia", "ort", "models", "output"].every(key => values[key]),
    "Usage: --inputs PCM_PATHS_JSON --corpus-manifest CORPUS_JSON --essentia PACKAGE_DIRECTORY --ort PACKAGE_DIRECTORY --models DIRECTORY --output NEW_JSON");
  const required = (key: string): string => { const value = values[key]; assert(value); return value; };
  return generateEvidence({ inputs: required("inputs"), corpusManifest: required("corpus-manifest"),
    essentia: required("essentia"), ort: required("ort"),
    models: required("models"), output: required("output") });
}

if (process.argv[1] && import.meta.url === pathToFileURL(process.argv[1]).href) {
  main(process.argv.slice(2)).then(result => console.log(JSON.stringify(result))).catch(() => {
    console.error("Jev audio evidence export failed. Check pinned artifacts, hashes, coverage and explicit paths. No provider request was made.");
    process.exitCode = 1;
  });
}
