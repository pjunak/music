// Offline whole-track qualification. No application imports, downloads or mood assignments.
import assert from "node:assert/strict";
import { createHash } from "node:crypto";
import fs from "node:fs";
import { setImmediate } from "node:timers/promises";
import { parseArgs } from "node:util";
import { pathToFileURL } from "node:url";
import { checkCancelled, frameCount, FRAME_SIZE, HOP, MAX_SAMPLES, PATCH_FRAMES, PATCH_HOP,
  readInputs, withReferenceRuntime } from "./effnet-reference.mjs";

const MEL_BANDS = 96;
const AGGREGATION = "nearest_patch_center_time_partition/v1";

// Each sample contributes once. These weights are a summary convention, not mood boundaries.
export function* patchSchedule(sampleCount) {
  const count = frameCount(sampleCount), last = count - PATCH_FRAMES;
  assert(last >= 0, "Audio is too short for one complete patch");
  const center = start => (start + (PATCH_FRAMES - 1) / 2) * HOP;
  let start = 0, previous = null;
  while (true) {
    const next = start === last ? null : Math.min(start + PATCH_HOP, last);
    const supportStart = start * HOP - FRAME_SIZE / 2;
    const supportEnd = supportStart + (PATCH_FRAMES - 1) * HOP + FRAME_SIZE;
    yield {
      frame_start: start,
      valid_start_sample: Math.max(0, supportStart), valid_end_sample: Math.min(sampleCount, supportEnd),
      padding_start_samples: Math.max(0, -supportStart), padding_end_samples: Math.max(0, supportEnd - sampleCount),
      weight_start_sample: previous === null ? 0 : (center(previous) + center(start)) / 2,
      weight_end_sample: next === null ? sampleCount : (center(start) + center(next)) / 2,
    };
    if (next === null) break;
    previous = start; start = next;
  }
}

function sameSnapshot(left, right) {
  return ["dev", "ino", "size", "mtimeNs", "ctimeNs"].every(key => left[key] === right[key]);
}

export class PcmFrames {
  constructor(file) {
    this.file = file;
    this.fd = fs.openSync(file, "r");
    try {
      this.before = fs.fstatSync(this.fd, { bigint: true });
      const size = Number(this.before.size);
      assert(this.before.isFile() && size > 0 && size % 4 === 0 && size <= MAX_SAMPLES * 4, "Invalid bounded PCM file");
      this.samples = size / 4;
      // Refuse short recordings before any inference; never fabricate repeated frames.
      patchSchedule(this.samples).next();
      this.count = frameCount(this.samples);
      this.hash = createHash("sha256");
      this.consumed = 0;
      this.finished = false;
    } catch (error) { this.close(); throw error; }
  }
  *frames(signal) {
    const bytes = Buffer.alloc(HOP * 4), frame = new Float32Array(FRAME_SIZE);
    for (let i = 0; i < this.count; i++) {
      checkCancelled(signal);
      frame.copyWithin(0, HOP); frame.fill(0, HOP);
      const needed = Math.min(HOP, this.samples - this.consumed) * 4;
      let used = 0;
      while (used < needed) {
        const read = fs.readSync(this.fd, bytes, used, needed - used, null);
        assert(read > 0, "PCM was truncated during streaming");
        used += read;
      }
      this.hash.update(bytes.subarray(0, used));
      for (let j = 0; j < used / 4; j++) {
        const value = bytes.readFloatLE(j * 4);
        assert(Number.isFinite(value), "Non-finite PCM");
        frame[HOP + j] = value;
      }
      this.consumed += used / 4;
      yield frame; // Reused buffer; the consumer must finish before requesting the next frame.
    }
    assert.equal(this.consumed, this.samples);
    assert.equal(fs.readSync(this.fd, bytes, 0, 1, null), 0, "PCM grew during streaming");
    assert(sameSnapshot(this.before, fs.fstatSync(this.fd, { bigint: true })) &&
      sameSnapshot(this.before, fs.statSync(this.file, { bigint: true })), "PCM changed during streaming");
    this.digest = this.hash.digest("hex");
    this.finished = true;
  }
  close() {
    if (this.fd !== undefined) { fs.closeSync(this.fd); this.fd = undefined; }
  }
}

export class TrackSummary {
  constructor() {
    this.end = 0;
    this.patches = 0;
    this.heads = Object.fromEntries([["mood", 56], ["instrument", 40]].map(([name, length]) =>
      [name, { mean: new Float64Array(length), m2: new Float64Array(length), max: new Float64Array(length) }]));
  }
  add(patch, scores) {
    const begin = patch.weight_start_sample, end = patch.weight_end_sample;
    assert(Number.isSafeInteger(begin) && Number.isSafeInteger(end) && begin === this.end && end > begin,
      "Weights must partition valid time exactly");
    for (const [name, head] of Object.entries(this.heads)) {
      assert(scores[name]?.length === head.mean.length &&
        scores[name].every(value => Number.isFinite(value) && value >= 0 && value <= 1), "Invalid head scores");
    }
    const weight = end - begin;
    for (const [name, head] of Object.entries(this.heads)) {
      scores[name].forEach((value, i) => {
        const delta = value - head.mean[i];
        head.mean[i] += weight / end * delta;
        head.m2[i] += weight * delta * (value - head.mean[i]);
        head.max[i] = Math.max(head.max[i], value);
      });
    }
    this.end = end;
    this.patches++;
  }
  finish(samples) {
    assert(this.end === samples && this.patches > 0, "Incomplete track summary");
    return Object.fromEntries(Object.entries(this.heads).map(([name, head]) => [name, {
      mean: Array.from(head.mean), stddev: Array.from(head.m2, value => Math.sqrt(Math.max(0, value / samples))),
      max: Array.from(head.max),
    }]));
  }
}

export async function streamTrack(file, runtime, { signal, onPatch = () => {} } = {}) {
  checkCancelled(signal);
  const input = new PcmFrames(file), started = performance.now();
  try {
    const schedule = patchSchedule(input.samples);
    let selected = schedule.next(), frames = 0;
    const mel = new Float32Array(PATCH_FRAMES * MEL_BANDS), summary = new TrackSummary();
    for (const frame of input.frames(signal)) {
      const bands = runtime.transform(frame);
      assert(bands.length === MEL_BANDS && bands.every(Number.isFinite), "Invalid reference features");
      mel.copyWithin(0, MEL_BANDS); mel.set(bands, mel.length - MEL_BANDS);
      frames++;
      if (!selected.done && frames === selected.value.frame_start + PATCH_FRAMES) {
        // ORT's WASM promises can resolve on microtasks; yield so OS cancellation is observed.
        await setImmediate(); checkCancelled(signal);
        const scores = await runtime.infer(mel, signal);
        checkCancelled(signal);
        assert(scores.embeddings?.length === 1280 && scores.embeddings.every(Number.isFinite), "Invalid embeddings");
        summary.add(selected.value, scores);
        await onPatch({ ...selected.value, ...scores });
        checkCancelled(signal);
        selected = schedule.next();
      }
    }
    checkCancelled(signal);
    assert(input.finished && selected.done && frames === input.count, "Incomplete stream");
    return {
      decoded_samples: input.samples, frame_count: frames, pcm_sha256: input.digest, patches: summary.patches,
      covered_samples: summary.end, summary: summary.finish(input.samples), elapsed_ms: performance.now() - started,
    };
  } finally { input.close(); }
}

export async function generateStreamReference(options, withRuntime = withReferenceRuntime) {
  const inputs = readInputs(options.inputs);
  const output = fs.openSync(options.output, "wx");
  const write = record => fs.writeFileSync(output, JSON.stringify(record) + "\n");
  const started = performance.now(), cpuStart = process.cpuUsage();
  let tracks = 0, patches = 0;
  try {
    await withRuntime(options, async runtime => {
      write({
        record_type: "header", schema_version: "effnet-stream-reference/v2", ...runtime.header,
        tracks: inputs.length, aggregation: AGGREGATION,
        scope: "Complete-track common-PCM ONNX reference; not decoder, original TensorFlow or mood-quality acceptance",
        limits: { tracks: 32, samples_per_track: MAX_SAMPLES, model_batch: 1, retained_mel_frames: PATCH_FRAMES,
          retained_embeddings: 1, retained_pcm_samples: FRAME_SIZE, pcm_read_buffer_bytes: HOP * 4 },
      });
      for (let index = 0; index < inputs.length; index++) {
        const track = await streamTrack(inputs[index], runtime, {
          signal: options.signal, onPatch: patch => { write({ record_type: "patch", index, ...patch }); patches++; },
        });
        write({ record_type: "track", index, ...track }); tracks++;
      }
    });
    checkCancelled(options.signal);
    const cpu = process.cpuUsage(cpuStart);
    const result = { status: "complete", tracks, patches, elapsed_ms: performance.now() - started,
      cpu_ms: (cpu.user + cpu.system) / 1000, peak_process_rss_bytes: process.resourceUsage().maxRSS * 1024 };
    // Only completed tracks and successfully released runtime sessions can complete the run.
    write({ record_type: "complete", ...result });
    return result;
  } catch (error) {
    if (error?.name !== "AbortError") throw error;
    const result = { status: "cancelled", tracks, patches };
    write({ record_type: "cancelled", ...result });
    return result;
  } finally { fs.closeSync(output); }
}

export async function main(args) {
  const { values } = parseArgs({
    args, options: Object.fromEntries(["inputs", "essentia", "ort", "models", "output"].map(key => [key, { type: "string" }])),
  });
  assert(["inputs", "essentia", "ort", "models", "output"].every(key => values[key]),
    "Usage: --inputs PCM_PATHS_JSON --essentia PACKAGE_DIRECTORY --ort PACKAGE_DIRECTORY --models DIRECTORY --output NEW_JSONL");
  const controller = new AbortController(), cancel = () => controller.abort();
  process.on("SIGINT", cancel); process.on("SIGTERM", cancel);
  try { return await generateStreamReference({ ...values, signal: controller.signal }); }
  finally { process.off("SIGINT", cancel); process.off("SIGTERM", cancel); }
}

if (process.argv[1] && import.meta.url === pathToFileURL(process.argv[1]).href) {
  main(process.argv.slice(2)).then(result => {
    console.log(JSON.stringify(result));
    if (result.status === "cancelled") process.exitCode = 130;
  }).catch(() => {
    console.error("EffNet stream reference failed. Check explicit arguments, pinned artifacts and bounded finite PCM. Incomplete output is not a passing reference.");
    process.exitCode = 1;
  });
}
