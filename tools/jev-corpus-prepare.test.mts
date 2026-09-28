import assert from "node:assert/strict";
import { createHash } from "node:crypto";
import fs from "node:fs";
import { tmpdir } from "node:os";
import path from "node:path";
import { test } from "node:test";
import type { TestContext } from "node:test";
import { readCorpusManifest } from "./jev-audio-evidence.mts";
import { prepareCorpus } from "./jev-corpus-prepare.mts";
import type { CorpusPrepareDependencies, CorpusPrepareOptions, ProcessRunner } from "./jev-corpus-prepare.mts";

const hash = (bytes: Buffer): string => createHash("sha256").update(bytes).digest("hex");

function temporary(t: TestContext): string {
  const directory = fs.mkdtempSync(path.join(tmpdir(), "jev-corpus-prepare-"));
  t.after(() => fs.rmSync(directory, { recursive: true, force: true }));
  return directory;
}

function fixture(t: TestContext, names = ["alpha.m4a", "beta.m4a"]) {
  const root = temporary(t), sources = names.map((name, index) => {
    const file = path.join(root, name);
    fs.writeFileSync(file, Buffer.from(`private-audio-${index}`));
    return file;
  });
  const list = path.join(root, "sources.json");
  fs.writeFileSync(list, JSON.stringify([...sources].reverse()));
  const executable = path.join(root, "fake.exe");
  fs.writeFileSync(executable, "fake");
  const runtime = path.join(root, "runtime");
  fs.mkdirSync(runtime);
  const output = path.join(root, "output");
  const options: CorpusPrepareOptions = { sources: list, outputDirectory: output, ffmpeg: executable,
    ffprobe: executable, analyzer: executable, essentia: runtime, ort: runtime, models: runtime };
  return { root, sources: [...sources].reverse(), output, options };
}

function successfulDependencies(invocations: Array<{ executable: string; args: readonly string[] }> = []): CorpusPrepareDependencies {
  const runProcess: ProcessRunner = (executable, args) => {
    invocations.push({ executable, args });
    if (args[0] === "pilot-analyze") {
      const paths = JSON.parse(fs.readFileSync(args.at(-2)!, "utf8")) as string[];
      fs.writeFileSync(args.at(-1)!, JSON.stringify({ schema_version: "jev-private-corpus/v1", recordings: paths.map((file, index) => ({
        source_path: process.platform === "win32" ? `\\\\?\\${file}` : file,
        file_sha256: hash(fs.readFileSync(file)), input: { track_id: index + 1 },
      })) }), { flag: "wx" });
    } else {
      const source = args[args.indexOf("-i") + 1], output = args.at(-1)!;
      const marker = fs.readFileSync(source)[14] ?? 1;
      const pcm = Buffer.alloc(33_000 * 4);
      pcm.writeFloatLE(marker / 255, 0);
      fs.writeFileSync(output, pcm, { flag: "wx" });
    }
    return { status: 0, signal: null };
  };
  return {
    runProcess,
    async streamReference(options) {
      const inputs = JSON.parse(fs.readFileSync(options.inputs, "utf8")) as string[];
      const records = [JSON.stringify({ record_type: "header", tracks: inputs.length }),
        ...inputs.map((file, index) => JSON.stringify({ record_type: "track", index,
          pcm_sha256: hash(fs.readFileSync(file)) })),
        JSON.stringify({ record_type: "complete", status: "complete", tracks: inputs.length })];
      fs.writeFileSync(options.output, records.join("\n") + "\n", { flag: "wx" });
      return { status: "complete" as const, tracks: inputs.length, patches: inputs.length,
        elapsed_ms: 1, cpu_ms: 1, peak_process_rss_bytes: 1 };
    },
    prepareManifest: async options => {
      const { prepareCorpusManifest } = await import("./jev-audio-evidence.mts");
      return prepareCorpusManifest(options);
    },
    async exportEvidence(options) {
      const inputs = JSON.parse(fs.readFileSync(options.inputs, "utf8")) as string[];
      readCorpusManifest(options.corpusManifest, inputs);
      fs.writeFileSync(options.output, JSON.stringify({ schema_version: "test", recordings: inputs.length }), { flag: "wx" });
      return { recordings: inputs.length, patches: inputs.length };
    },
  };
}

void test("preparation preserves explicit source/PCM order and immutable hash pairing", async t => {
  const value = fixture(t), invocations: Array<{ executable: string; args: readonly string[] }> = [];
  const result = await prepareCorpus(value.options, successfulDependencies(invocations));
  const manifest = JSON.parse(fs.readFileSync(path.join(value.output, "corpus-manifest.json"), "utf8")) as
    Array<{ file: string; sha256: string; pcm_file: string; pcm_sha256: string }>;
  assert.equal(result.recordings, 2);
  assert.deepEqual(manifest.map(item => item.file), value.sources);
  assert.deepEqual(manifest.map(item => item.sha256), value.sources.map(file => hash(fs.readFileSync(file))));
  assert.deepEqual(manifest.map(item => path.basename(item.pcm_file)), ["track-0.f32le", "track-1.f32le"]);
  assert.deepEqual(manifest.map(item => item.pcm_sha256), manifest.map(item => hash(fs.readFileSync(item.pcm_file))));
  assert.equal(invocations.length, 3);
  assert.deepEqual(invocations[0].args.slice(0, 4), ["-v", "error", "-nostdin", "-filter_threads"]);
  assert.equal(invocations[2].args[0], "pilot-analyze");
  assert.deepEqual(JSON.parse(fs.readFileSync(path.join(value.output, "source-before.json"), "utf8")),
    JSON.parse(fs.readFileSync(path.join(value.output, "source-after.json"), "utf8")));
});

void test("a source changing during decode fails before analysis or inference", async t => {
  const value = fixture(t, ["changing.m4a"]), calls: string[] = [];
  const dependencies = successfulDependencies();
  dependencies.runProcess = (_executable, args) => {
    calls.push(String(args[0]));
    const source = args[args.indexOf("-i") + 1], output = args.at(-1)!;
    fs.writeFileSync(output, Buffer.alloc(33_000 * 4), { flag: "wx" });
    fs.appendFileSync(source, "changed");
    return { status: 0, signal: null };
  };
  await assert.rejects(prepareCorpus(value.options, dependencies), /Source files changed during preparation/);
  assert.deepEqual(calls, ["-v"]);
  const failure = JSON.parse(fs.readFileSync(path.join(value.output, "failure.json"), "utf8")) as Record<string, unknown>;
  assert.equal(failure.stage, "decode");
});

void test("a subprocess failure is explicit and leaves a non-passing partial output", async t => {
  const value = fixture(t, ["failure.m4a"]), dependencies = successfulDependencies();
  dependencies.runProcess = () => ({ status: 9, signal: null, stderr: "private detail" });
  await assert.rejects(prepareCorpus(value.options, dependencies), /Decode 1 failed with exit code 9/);
  assert(!fs.existsSync(path.join(value.output, "preparation-summary.json")));
  const failure = JSON.parse(fs.readFileSync(path.join(value.output, "failure.json"), "utf8")) as Record<string, unknown>;
  assert.equal(failure.status, "failed");
  assert.equal(failure.stage, "decode");
  assert(!String(failure.error).includes("private detail"));
});
