import { spawnSync } from "node:child_process";
import { lstatSync, readdirSync, realpathSync, writeFileSync } from "node:fs";
import { delimiter, dirname, extname, join, resolve } from "node:path";
import { fileURLToPath, pathToFileURL } from "node:url";

const RECORD_PREFIX = "VOICE_PROBE_JSON ";
const EXPECTED_MODEL_SHA256 = "b734bca3fc99257cf0088211b44bd36e8a26fbb1f9ce67e1e97d39f188094b0a";
const MAX_SCORE_TOLERANCE = 0.05;
const MAX_COVERAGE_TOLERANCE = 0.10;
const MINIMUM_REPRESENTATIVE_TRACKS = 6;
const AUDIO_EXTENSIONS = new Set([".aac", ".flac", ".m4a", ".mp3", ".ogg", ".opus", ".wav", ".wma"]);
const SCRIPT_DIRECTORY = dirname(fileURLToPath(import.meta.url));
const REPOSITORY_ROOT = resolve(SCRIPT_DIRECTORY, "..", "..");

function usage() {
  return [
    "usage: node .github/scripts/voice-differential.mjs \\",
    "  --python backend/.venv/bin/python --model /models/voice_instrumental-musicnn-msd-2.pb \\",
    "  --ffmpeg /usr/bin/ffmpeg --corpus /private/representative-audio \\",
    "  [--report voice-differential.md] [--score-tolerance 0.05] \\",
    "  [--coverage-tolerance 0.10] [--minimum-tracks 6] [--cargo cargo]",
  ].join("\n");
}

export function parseOptions(arguments_) {
  const values = new Map();
  const known = new Set([
    "--python",
    "--model",
    "--ffmpeg",
    "--corpus",
    "--report",
    "--score-tolerance",
    "--coverage-tolerance",
    "--minimum-tracks",
    "--cargo",
  ]);
  for (let index = 0; index < arguments_.length; index += 2) {
    const flag = arguments_[index];
    const value = arguments_[index + 1];
    if (!known.has(flag) || value === undefined || value.startsWith("--")) {
      throw new Error(usage());
    }
    if (values.has(flag)) throw new Error(`${flag} may be specified only once`);
    values.set(flag, value);
  }
  for (const flag of ["--python", "--model", "--ffmpeg", "--corpus"]) {
    if (!values.has(flag)) throw new Error(`${flag} is required\n\n${usage()}`);
  }
  const scoreTolerance = finiteNumber(values.get("--score-tolerance") ?? String(MAX_SCORE_TOLERANCE), "--score-tolerance");
  const coverageTolerance = finiteNumber(values.get("--coverage-tolerance") ?? String(MAX_COVERAGE_TOLERANCE), "--coverage-tolerance");
  const minimumTracks = finiteNumber(values.get("--minimum-tracks") ?? String(MINIMUM_REPRESENTATIVE_TRACKS), "--minimum-tracks");
  if (scoreTolerance < 0 || scoreTolerance > MAX_SCORE_TOLERANCE) {
    throw new Error(`--score-tolerance may be stricter than, but not exceed, ${MAX_SCORE_TOLERANCE}`);
  }
  if (coverageTolerance < 0 || coverageTolerance > MAX_COVERAGE_TOLERANCE) {
    throw new Error(`--coverage-tolerance may be stricter than, but not exceed, ${MAX_COVERAGE_TOLERANCE}`);
  }
  if (!Number.isInteger(minimumTracks) || minimumTracks < MINIMUM_REPRESENTATIVE_TRACKS || minimumTracks > 512) {
    throw new Error(`--minimum-tracks must be an integer from ${MINIMUM_REPRESENTATIVE_TRACKS} through 512`);
  }
  return {
    python: values.get("--python"),
    model: resolve(values.get("--model")),
    ffmpeg: values.get("--ffmpeg"),
    corpus: realpathSync(values.get("--corpus")),
    report: resolve(values.get("--report") ?? "voice-differential.md"),
    scoreTolerance,
    coverageTolerance,
    minimumTracks,
    cargo: values.get("--cargo") ?? "cargo",
  };
}

function finiteNumber(value, flag) {
  const parsed = Number(value);
  if (!Number.isFinite(parsed)) throw new Error(`${flag} must be a finite number`);
  return parsed;
}

function collectAudioFiles(root) {
  const files = [];
  let skippedSymlinks = 0;
  function visit(directory) {
    const entries = readdirSync(directory, { withFileTypes: true })
      .sort((left, right) => left.name < right.name ? -1 : left.name > right.name ? 1 : 0);
    for (const entry of entries) {
      const path = join(directory, entry.name);
      const stat = lstatSync(path);
      if (stat.isSymbolicLink()) {
        skippedSymlinks += 1;
      } else if (stat.isDirectory()) {
        visit(path);
      } else if (stat.isFile() && AUDIO_EXTENSIONS.has(extname(entry.name).toLowerCase())) {
        files.push(path);
      }
    }
  }
  visit(root);
  return { files, skippedSymlinks };
}

function run(command, arguments_, options = {}) {
  const result = spawnSync(command, arguments_, {
    cwd: REPOSITORY_ROOT,
    encoding: "utf8",
    maxBuffer: 64 * 1_024 * 1_024,
    ...options,
  });
  if (result.error) throw result.error;
  if (result.status !== 0) {
    const detail = String(result.stderr ?? "").trim().slice(-4_000);
    throw new Error(`${command} exited with status ${result.status}${detail ? `:\n${detail}` : ""}`);
  }
  return String(result.stdout ?? "");
}

function parseRecords(output, expectedCount, owner) {
  const records = output
    .split(/\r?\n/u)
    .filter((line) => line.startsWith(RECORD_PREFIX))
    .map((line) => JSON.parse(line.slice(RECORD_PREFIX.length)));
  if (records.length !== expectedCount) {
    throw new Error(`${owner} emitted ${records.length} records for ${expectedCount} tracks`);
  }
  const byIndex = new Map();
  for (const record of records) {
    if (record.schema_version !== "voice-probe/v1" || !Number.isInteger(record.index)) {
      throw new Error(`${owner} emitted an invalid voice-probe record`);
    }
    if (record.index < 0 || record.index >= expectedCount || byIndex.has(record.index)) {
      throw new Error(`${owner} emitted a duplicate or out-of-range record index`);
    }
    byIndex.set(record.index, record);
  }
  return Array.from({ length: expectedCount }, (_, index) => byIndex.get(index));
}

function qualitativeLabel(score, coverage) {
  if (score >= 0.65 && coverage >= 0.6) return "mostly voice";
  if (score >= 0.55 && coverage >= 0.2) return "partial voice";
  if (score <= 0.35 && coverage <= 0.2) return "mostly instrumental";
  return "mixed or uncertain";
}

export function compareRecords(pythonRecords, rustRecords, options) {
  if (pythonRecords.length !== rustRecords.length) throw new Error("voice record counts differ");
  const rows = [];
  const failures = [];
  for (let index = 0; index < pythonRecords.length; index += 1) {
    const python = pythonRecords[index];
    const rust = rustRecords[index];
    const identity = options.identities?.[index] ?? String(index + 1);
    let contractMatches = true;
    for (const [owner, record] of [["Python", python], ["Rust", rust]]) {
      if (record?.status !== "classified") {
        contractMatches = false;
        failures.push(`${identity}: ${owner} status is ${record?.status ?? "missing"}`);
      }
      if (record?.model_sha256 !== EXPECTED_MODEL_SHA256) {
        contractMatches = false;
        failures.push(`${identity}: ${owner} model checksum is not the pinned graph`);
      }
      if (!Number.isInteger(record?.prediction_windows) || record.prediction_windows < 1) {
        contractMatches = false;
        failures.push(`${identity}: ${owner} prediction-window count is missing or invalid`);
      }
      if (typeof record?.elapsed_seconds !== "number" || !Number.isFinite(record.elapsed_seconds) || record.elapsed_seconds < 0) {
        contractMatches = false;
        failures.push(`${identity}: ${owner} elapsed time is missing or invalid`);
      }
    }
    const pythonScore = python?.voice_score;
    const rustScore = rust?.voice_score;
    const pythonCoverage = python?.vocal_coverage;
    const rustCoverage = rust?.vocal_coverage;
    const numeric = [pythonScore, rustScore, pythonCoverage, rustCoverage]
      .every((value) => typeof value === "number" && Number.isFinite(value) && value >= 0 && value <= 1);
    if (!numeric) {
      failures.push(`${identity}: one or more bounded numeric fields are missing`);
      rows.push({ identity, python, rust, scoreDelta: Number.NaN, coverageDelta: Number.NaN, passed: false });
      continue;
    }
    const scoreDelta = Math.abs(pythonScore - rustScore);
    const coverageDelta = Math.abs(pythonCoverage - rustCoverage);
    const windowsMatch = python.prediction_windows === rust.prediction_windows;
    const labelsMatch = qualitativeLabel(pythonScore, pythonCoverage) === qualitativeLabel(rustScore, rustCoverage);
    if (scoreDelta > options.scoreTolerance) failures.push(`${identity}: voice-score delta ${scoreDelta.toFixed(5)} exceeds ${options.scoreTolerance}`);
    if (coverageDelta > options.coverageTolerance) failures.push(`${identity}: coverage delta ${coverageDelta.toFixed(5)} exceeds ${options.coverageTolerance}`);
    if (!windowsMatch) failures.push(`${identity}: prediction-window counts differ`);
    if (!labelsMatch) failures.push(`${identity}: qualitative evidence buckets differ`);
    rows.push({
      identity,
      python,
      rust,
      scoreDelta,
      coverageDelta,
      passed: contractMatches && scoreDelta <= options.scoreTolerance && coverageDelta <= options.coverageTolerance && windowsMatch && labelsMatch,
    });
  }
  return { rows, failures };
}

function median(values) {
  const sorted = [...values].sort((left, right) => left - right);
  const middle = Math.floor(sorted.length / 2);
  return sorted.length % 2 === 0 ? (sorted[middle - 1] + sorted[middle]) / 2 : sorted[middle];
}

function fixed(value, digits = 5) {
  return Number.isFinite(value) ? value.toFixed(digits) : "n/a";
}

export function renderReport(comparison, metadata) {
  const scoreDeltas = comparison.rows.map((row) => row.scoreDelta).filter(Number.isFinite);
  const coverageDeltas = comparison.rows.map((row) => row.coverageDelta).filter(Number.isFinite);
  const pythonTimes = comparison.rows.map((row) => row.python?.elapsed_seconds).filter(Number.isFinite);
  const rustTimes = comparison.rows.map((row) => row.rust?.elapsed_seconds).filter(Number.isFinite);
  const status = comparison.failures.length === 0 ? "PASS" : "FAIL";
  const lines = [
    "# Essentia/Rust voice differential",
    "",
    `- Status: **${status}**`,
    `- Git commit: \`${metadata.commit}\``,
    `- Tracks: ${comparison.rows.length}; source paths and filenames omitted`,
    `- Skipped symlinks: ${metadata.skippedSymlinks}`,
    `- Model SHA-256: \`${EXPECTED_MODEL_SHA256}\``,
    `- Voice-score tolerance: ${metadata.scoreTolerance}`,
    `- Vocal-coverage tolerance: ${metadata.coverageTolerance}`,
    `- Maximum absolute score delta: ${fixed(Math.max(...scoreDeltas))}`,
    `- Maximum absolute coverage delta: ${fixed(Math.max(...coverageDeltas))}`,
    `- Median warm Python inference: ${fixed(median(pythonTimes), 3)} s`,
    `- Median warm Rust inference: ${fixed(median(rustTimes), 3)} s`,
    "- Additional gate: exact prediction-window count and unchanged qualitative evidence bucket per track",
    "",
    "| Opaque track | Python score | Rust score | Score delta | Python coverage | Rust coverage | Coverage delta | Windows P/R | Result |",
    "|---|---:|---:|---:|---:|---:|---:|---:|---|",
  ];
  for (const row of comparison.rows) {
    lines.push(`| \`${row.identity}\` | ${fixed(row.python?.voice_score)} | ${fixed(row.rust?.voice_score)} | ${fixed(row.scoreDelta)} | ${fixed(row.python?.vocal_coverage)} | ${fixed(row.rust?.vocal_coverage)} | ${fixed(row.coverageDelta)} | ${row.python?.prediction_windows ?? "?"}/${row.rust?.prediction_windows ?? "?"} | ${row.passed ? "PASS" : "FAIL"} |`);
  }
  if (comparison.failures.length > 0) {
    lines.push("", "## Failed gates", "", ...comparison.failures.map((failure) => `- ${failure}`));
  }
  lines.push(
    "",
    "The corpus remained local. Opaque ordered labels correlate failures without hashing or publishing paths and filenames.",
    "",
  );
  return lines.join("\n");
}

function pythonProbeCode() {
  return String.raw`
import json
import sys
import time
from pathlib import Path

from app.assistant.voice_analysis import analyze_voice

paths = json.load(sys.stdin)
warmup = analyze_voice(Path(paths[0]))
if warmup.summary.get("status") != "classified":
    raise SystemExit("voice warmup did not classify")
for index, path_text in enumerate(paths):
    started = time.perf_counter()
    result = analyze_voice(Path(path_text))
    elapsed = time.perf_counter() - started
    record = {
        "schema_version": "voice-probe/v1",
        "index": index,
        "status": result.summary.get("status"),
        "voice_score": result.summary.get("voice_probability"),
        "vocal_coverage": result.summary.get("vocal_coverage"),
        "prediction_windows": result.stage.get("prediction_windows"),
        "model_sha256": result.stage.get("model_sha256"),
        "elapsed_seconds": elapsed,
    }
    print("VOICE_PROBE_JSON " + json.dumps(record, separators=(",", ":")), flush=True)
`;
}

function main() {
  if (process.argv.length === 3 && ["--help", "-h"].includes(process.argv[2])) {
    process.stdout.write(`${usage()}\n`);
    return;
  }
  const options = parseOptions(process.argv.slice(2));
  if (run("git", ["status", "--porcelain", "--untracked-files=all"]).trim()) {
    throw new Error("voice acceptance requires a clean Git worktree so the recorded commit identifies the tested code");
  }
  const corpusStat = lstatSync(options.corpus);
  if (!corpusStat.isDirectory()) throw new Error("--corpus must resolve to a directory");
  const { files, skippedSymlinks } = collectAudioFiles(options.corpus);
  if (files.length < options.minimumTracks) {
    throw new Error(`corpus contains ${files.length} audio tracks; at least ${options.minimumTracks} are required`);
  }
  if (files.length > 512) throw new Error("corpus contains more than the 512-track safety limit");
  const input = `${JSON.stringify(files)}\n`;

  const build = spawnSync(options.cargo, ["build", "--release", "--locked", "-p", "music-analysis", "--bin", "music-voice-probe"], {
    cwd: REPOSITORY_ROOT,
    stdio: "inherit",
  });
  if (build.error) throw build.error;
  if (build.status !== 0) throw new Error(`Rust voice-probe build exited with status ${build.status}`);

  const pythonPath = join(REPOSITORY_ROOT, "backend");
  const pythonOutput = run(options.python, ["-c", pythonProbeCode()], {
    input,
    env: {
      ...process.env,
      ASSISTANT_VOICE_MODEL_PATH: options.model,
      PYTHONPATH: `${pythonPath}${process.env.PYTHONPATH ? delimiter + process.env.PYTHONPATH : ""}`,
      TF_CPP_MIN_LOG_LEVEL: "3",
    },
  });
  const binary = join(REPOSITORY_ROOT, "target", "release", process.platform === "win32" ? "music-voice-probe.exe" : "music-voice-probe");
  const rustOutput = run(binary, ["--model", options.model, "--ffmpeg", options.ffmpeg, "--warmup"], { input });
  const pythonRecords = parseRecords(pythonOutput, files.length, "Python");
  const rustRecords = parseRecords(rustOutput, files.length, "Rust");
  const identities = files.map((_, index) => `track-${String(index + 1).padStart(3, "0")}`);
  const comparison = compareRecords(pythonRecords, rustRecords, { ...options, identities });
  const commit = run("git", ["rev-parse", "HEAD"]).trim();
  const report = renderReport(comparison, {
    commit,
    skippedSymlinks,
    scoreTolerance: options.scoreTolerance,
    coverageTolerance: options.coverageTolerance,
  });
  writeFileSync(options.report, report, { encoding: "utf8", flag: "wx" });
  process.stdout.write(report);
  if (comparison.failures.length > 0) throw new Error("voice differential failed; inspect the path-free report");
}

const invokedPath = process.argv[1] ? pathToFileURL(resolve(process.argv[1])).href : "";
if (invokedPath === import.meta.url) {
  try {
    main();
  } catch (error) {
    process.stderr.write(`voice-differential: ${error instanceof Error ? error.message : String(error)}\n`);
    process.exitCode = 1;
  }
}
