// Offline preparation of explicit private audio for bounded Jev experiments. No network access.
import assert from "node:assert/strict";
import { spawnSync } from "node:child_process";
import { createHash } from "node:crypto";
import fs from "node:fs";
import path from "node:path";
import { parseArgs } from "node:util";
import { pathToFileURL } from "node:url";
import { generateEvidence, prepareCorpusManifest } from "./jev-audio-evidence.mts";
import { frameCount, MAX_SAMPLES, PATCH_FRAMES, readBounded, SAMPLE_RATE } from "./effnet-reference.mts";
import { generateStreamReference } from "./effnet-stream.mts";

const HEX_SHA256 = /^[0-9a-f]{64}$/;
const PROCESS_OUTPUT_LIMIT = 1024 * 1024;
const DECODE_TIMEOUT_MS = 20 * 60 * 1000;
const ANALYZE_TIMEOUT_MS = 60 * 60 * 1000;

export interface CorpusPrepareOptions {
  sources: string;
  outputDirectory: string;
  ffmpeg: string;
  ffprobe: string;
  analyzer: string;
  essentia: string;
  ort: string;
  models: string;
}

export interface ProcessResult {
  status: number | null;
  signal: NodeJS.Signals | null;
  error?: Error;
  stderr?: string;
}

export type ProcessRunner = (
  executable: string,
  args: readonly string[],
  options: { timeoutMs: number },
) => ProcessResult;

export interface CorpusPrepareDependencies {
  runProcess: ProcessRunner;
  streamReference: typeof generateStreamReference;
  prepareManifest: typeof prepareCorpusManifest;
  exportEvidence: typeof generateEvidence;
}

interface SourceSnapshot {
  index: number;
  source_id: string;
  file: string;
  bytes: number;
  modified_ms: number;
  sha256: string;
}

interface PcmObservation {
  index: number;
  source_id: string;
  samples: number;
  sample_rate: typeof SAMPLE_RATE;
  pcm_sha256: string;
  expected_frames: number;
}

const defaultDependencies: CorpusPrepareDependencies = {
  runProcess(executable, args, options) {
    const result = spawnSync(executable, [...args], {
      encoding: "utf8",
      maxBuffer: PROCESS_OUTPUT_LIMIT,
      shell: false,
      timeout: options.timeoutMs,
      windowsHide: true,
    });
    return {
      status: result.status,
      signal: result.signal,
      error: result.error,
      stderr: result.stderr,
    };
  },
  streamReference: generateStreamReference,
  prepareManifest: prepareCorpusManifest,
  exportEvidence: generateEvidence,
};

const sha256 = (bytes: Buffer): string => createHash("sha256").update(bytes).digest("hex");

function writeNewJson(file: string, value: unknown): void {
  fs.writeFileSync(file, JSON.stringify(value, null, 2) + "\n", { flag: "wx" });
}

function requireFile(value: string, label: string): string {
  const resolved = path.resolve(value);
  const stat = fs.statSync(resolved);
  assert(stat.isFile(), `${label} must be a regular file`);
  return resolved;
}

function requireDirectory(value: string, label: string): string {
  const resolved = path.resolve(value);
  const stat = fs.statSync(resolved);
  assert(stat.isDirectory(), `${label} must be a directory`);
  return resolved;
}

function readSourcePaths(file: string): string[] {
  const value: unknown = JSON.parse(readBounded(file, 1024 * 1024).toString("utf8"));
  assert(Array.isArray(value) && value.length > 0 && value.length <= 32 &&
    value.every(item => typeof item === "string" && path.isAbsolute(item)),
  "Expected 1-32 absolute source paths");
  const sourcePaths = value as string[];
  const canonical = sourcePaths.map(item => fs.realpathSync(item));
  assert.equal(new Set(canonical.map(item => path.normalize(item).toLowerCase())).size, canonical.length,
    "Source paths must be unique");
  return canonical;
}

function hashFile(file: string): { bytes: number; modified_ms: number; sha256: string } {
  const descriptor = fs.openSync(file, "r");
  try {
    const before = fs.fstatSync(descriptor, { bigint: true });
    assert(before.isFile() && before.size > 0 && before.size <= BigInt(Number.MAX_SAFE_INTEGER),
      "Source must be a nonempty bounded regular file");
    const digest = createHash("sha256"), buffer = Buffer.alloc(1024 * 1024);
    let bytes = 0, count;
    do {
      count = fs.readSync(descriptor, buffer, 0, buffer.length, null);
      if (count > 0) { digest.update(buffer.subarray(0, count)); bytes += count; }
    } while (count > 0);
    const after = fs.fstatSync(descriptor, { bigint: true });
    assert(before.dev === after.dev && before.ino === after.ino && before.size === after.size &&
      before.mtimeNs === after.mtimeNs && bytes === Number(before.size), "Source changed while hashing");
    return { bytes, modified_ms: Number(before.mtimeNs) / 1_000_000, sha256: digest.digest("hex") };
  } finally {
    fs.closeSync(descriptor);
  }
}

function snapshotSources(files: string[]): SourceSnapshot[] {
  return files.map((file, index) => ({ index, source_id: `source-${index + 1}`, file, ...hashFile(file) }));
}

function assertSourcesUnchanged(expected: SourceSnapshot[], actual: SourceSnapshot[]): void {
  assert.deepEqual(actual, expected, "Source files changed during preparation");
}

function runChecked(runner: ProcessRunner, executable: string, args: readonly string[], label: string, timeoutMs: number): void {
  const result = runner(executable, args, { timeoutMs });
  assert(!result.error && result.status === 0,
    `${label} failed${result.status === null ? "" : ` with exit code ${result.status}`}${result.signal ? ` (${result.signal})` : ""}`);
}

function validatePcm(file: string): PcmObservation["samples"] {
  const stat = fs.statSync(file);
  assert(stat.isFile() && stat.size > 0 && stat.size % 4 === 0 && stat.size <= MAX_SAMPLES * 4,
    "Decoder produced invalid bounded float32-le PCM");
  const samples = stat.size / 4;
  assert(frameCount(samples) >= PATCH_FRAMES, "Decoded audio is too short for one complete patch");
  return samples;
}

function comparablePath(value: string): string {
  let normalized = path.normalize(value);
  if (process.platform === "win32") {
    if (normalized.startsWith("\\\\?\\UNC\\")) normalized = `\\\\${normalized.slice(8)}`;
    else if (normalized.startsWith("\\\\?\\")) normalized = normalized.slice(4);
    normalized = normalized.toLowerCase();
  }
  return normalized;
}

function validatePhysicalCorpus(file: string, sources: SourceSnapshot[]): void {
  const value: unknown = JSON.parse(readBounded(file, 32 * 1024 * 1024).toString("utf8"));
  assert(value !== null && typeof value === "object" && !Array.isArray(value), "Invalid physical corpus");
  const corpus = value as Record<string, unknown>;
  assert.equal(corpus.schema_version, "jev-private-corpus/v1", "Unexpected physical corpus schema");
  assert(Array.isArray(corpus.recordings) && corpus.recordings.length === sources.length,
    "Physical corpus recording count mismatch");
  corpus.recordings.forEach((item, index) => {
    assert(item !== null && typeof item === "object" && !Array.isArray(item), "Invalid physical recording");
    const recording = item as Record<string, unknown>;
    const input = recording.input as Record<string, unknown> | undefined;
    assert.equal(comparablePath(String(recording.source_path)), comparablePath(sources[index].file),
      "Physical corpus source order mismatch");
    assert.equal(recording.file_sha256, sources[index].sha256, "Physical corpus source hash mismatch");
    assert.equal(input?.track_id, index + 1, "Physical corpus source identifier mismatch");
  });
}

function elapsed(start: number): number {
  return Math.round((performance.now() - start) * 1000) / 1000;
}

export async function prepareCorpus(
  options: CorpusPrepareOptions,
  dependencies: CorpusPrepareDependencies = defaultDependencies,
): Promise<Record<string, unknown>> {
  const sourceList = requireFile(options.sources, "Source list");
  const sources = readSourcePaths(sourceList);
  const ffmpeg = requireFile(options.ffmpeg, "FFmpeg");
  const ffprobe = requireFile(options.ffprobe, "FFprobe");
  const analyzer = requireFile(options.analyzer, "Analyzer");
  const essentia = requireDirectory(options.essentia, "Essentia package");
  const ort = requireDirectory(options.ort, "ONNX Runtime package");
  const models = requireDirectory(options.models, "Model directory");
  const output = path.resolve(options.outputDirectory);
  assert(!fs.existsSync(output), "Output directory must be new");
  const parent = path.dirname(output);
  assert(fs.statSync(parent).isDirectory(), "Output parent must exist");

  const before = snapshotSources(sources);
  assert.equal(new Set(before.map(item => item.sha256)).size, before.length, "Duplicate source audio content");
  fs.mkdirSync(output);
  const pcmDirectory = path.join(output, "pcm");
  fs.mkdirSync(pcmDirectory);
  const pathsFile = path.join(output, "source-paths.json");
  const beforeFile = path.join(output, "source-before.json");
  const pcmInputsFile = path.join(output, "pcm-inputs.json");
  const framingFile = path.join(output, "framing-observations.json");
  const physicalFile = path.join(output, "physical-corpus.json");
  const referenceFile = path.join(output, "effnet-stream-reference-v2.jsonl");
  const manifestFile = path.join(output, "corpus-manifest.json");
  const evidenceFile = path.join(output, "song-audio-predictions-v1.json");
  const afterFile = path.join(output, "source-after.json");
  const summaryFile = path.join(output, "preparation-summary.json");
  writeNewJson(pathsFile, sources);
  writeNewJson(beforeFile, before);

  let stage = "decode";
  const totalStarted = performance.now();
  try {
    const decodeStarted = performance.now(), pcmInputs: string[] = [], observations: PcmObservation[] = [];
    for (let index = 0; index < sources.length; index++) {
      const pcm = path.join(pcmDirectory, `track-${index}.f32le`);
      runChecked(dependencies.runProcess, ffmpeg, [
        "-v", "error", "-nostdin", "-filter_threads", "1", "-filter_complex_threads", "1", "-threads", "1",
        "-i", sources[index], "-map", "0:a:0", "-vn",
        "-af", "aresample=16000:out_chlayout=mono:rematrix_maxval=1",
        "-ac", "1", "-ar", String(SAMPLE_RATE), "-c:a", "pcm_f32le", "-f", "f32le", "-n", pcm,
      ], `Decode ${index + 1}`, DECODE_TIMEOUT_MS);
      assertSourcesUnchanged([before[index]], snapshotSources([sources[index]]).map(item => ({
        ...item, index, source_id: before[index].source_id,
      })));
      const samples = validatePcm(pcm), bytes = readBounded(pcm, MAX_SAMPLES * 4);
      const pcmSha256 = sha256(bytes);
      assert(HEX_SHA256.test(pcmSha256));
      pcmInputs.push(pcm);
      observations.push({ index, source_id: before[index].source_id, samples, sample_rate: SAMPLE_RATE,
        pcm_sha256: pcmSha256, expected_frames: frameCount(samples) });
    }
    const decodeMs = elapsed(decodeStarted);
    writeNewJson(pcmInputsFile, pcmInputs);
    writeNewJson(framingFile, observations);

    stage = "physical analysis";
    const physicalStarted = performance.now();
    runChecked(dependencies.runProcess, analyzer, ["pilot-analyze", "--ffmpeg", ffmpeg, "--ffprobe", ffprobe,
      pathsFile, physicalFile], "Physical analysis", ANALYZE_TIMEOUT_MS);
    const physicalMs = elapsed(physicalStarted);
    validatePhysicalCorpus(physicalFile, before);
    assertSourcesUnchanged(before, snapshotSources(sources));

    stage = "EffNet stream reference";
    const referenceStarted = performance.now();
    const reference = await dependencies.streamReference({ inputs: pcmInputsFile, essentia, ort, models, output: referenceFile });
    const referenceMs = elapsed(referenceStarted);
    assert.equal(reference.status, "complete", "EffNet stream reference did not complete");
    assert.equal(reference.tracks, sources.length, "EffNet stream reference count mismatch");
    assert("cpu_ms" in reference && typeof reference.cpu_ms === "number" &&
      "peak_process_rss_bytes" in reference && typeof reference.peak_process_rss_bytes === "number",
    "EffNet stream reference resource measurements are missing");

    stage = "corpus binding";
    await dependencies.prepareManifest({ inputs: pcmInputsFile, sourceManifest: beforeFile,
      framing: framingFile, reference: referenceFile, output: manifestFile });

    stage = "learned evidence export";
    const evidenceStarted = performance.now();
    const evidence = await dependencies.exportEvidence({ inputs: pcmInputsFile, corpusManifest: manifestFile,
      essentia, ort, models, output: evidenceFile });
    const evidenceMs = elapsed(evidenceStarted);
    assert.equal(evidence.recordings, sources.length, "Learned evidence count mismatch");

    stage = "final source verification";
    const after = snapshotSources(sources);
    assertSourcesUnchanged(before, after);
    writeNewJson(afterFile, after);
    const resource = process.resourceUsage();
    const summary = {
      schema_version: "jev-corpus-preparation/v1",
      status: "complete",
      recordings: sources.length,
      source_files_unchanged: true,
      learned_score_dimensions: { instrument: 40, mood_theme: 56, style: 400, total: 496 },
      coverage: "complete-track centered windows with explicit ending anchor and exact sample partition",
      failures: [],
      durations_ms: { decode: decodeMs, physical_analysis: physicalMs,
        effnet_stream_reference: referenceMs, learned_evidence_export: evidenceMs, total: elapsed(totalStarted) },
      resource: { wrapper_peak_process_rss_bytes: resource.maxRSS * 1024,
        reference_cpu_ms: reference.cpu_ms, reference_peak_process_rss_bytes: reference.peak_process_rss_bytes },
      tools: { ffmpeg, ffprobe, analyzer },
      runtimes: { essentia, ort, models },
      artifacts: { paths: pathsFile, source_before: beforeFile, pcm_inputs: pcmInputsFile,
        framing: framingFile, physical_corpus: physicalFile, stream_reference: referenceFile,
        corpus_manifest: manifestFile, learned_evidence: evidenceFile, source_after: afterFile },
      patches: { stream_reference: reference.patches, learned_evidence: evidence.patches },
    };
    writeNewJson(summaryFile, summary);
    return summary;
  } catch (error: unknown) {
    const failure = { schema_version: "jev-corpus-preparation/v1", status: "failed", stage,
      error: error instanceof Error ? error.message.slice(0, 1024) : "Unknown failure" };
    try { writeNewJson(path.join(output, "failure.json"), failure); } catch { /* retain the first failure */ }
    throw error;
  }
}

export async function main(args: string[]): Promise<Record<string, unknown>> {
  const keys = ["sources", "output-directory", "ffmpeg", "ffprobe", "analyzer", "essentia", "ort", "models"];
  const { values } = parseArgs({ args, options: Object.fromEntries(keys.map(key => [key, { type: "string" }])) });
  assert(keys.every(key => values[key]),
    "Usage: --sources SOURCE_PATHS_JSON --output-directory NEW_DIRECTORY --ffmpeg FILE --ffprobe FILE --analyzer FILE --essentia PACKAGE_DIRECTORY --ort PACKAGE_DIRECTORY --models DIRECTORY");
  const required = (key: string): string => { const value = values[key]; assert(value); return value; };
  return prepareCorpus({ sources: required("sources"), outputDirectory: required("output-directory"),
    ffmpeg: required("ffmpeg"), ffprobe: required("ffprobe"), analyzer: required("analyzer"),
    essentia: required("essentia"), ort: required("ort"), models: required("models") });
}

if (process.argv[1] && import.meta.url === pathToFileURL(process.argv[1]).href) {
  main(process.argv.slice(2)).then(result => console.log(JSON.stringify(result))).catch(() => {
    console.error("Jev corpus preparation failed. Inspect failure.json in the new output directory. No provider request was made.");
    process.exitCode = 1;
  });
}
