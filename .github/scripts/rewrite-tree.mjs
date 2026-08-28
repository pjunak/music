import { spawnSync } from "node:child_process";
import { createHash } from "node:crypto";
import { readFileSync } from "node:fs";
import { basename, dirname, resolve } from "node:path";
import { fileURLToPath, pathToFileURL } from "node:url";

const SCRIPT_DIRECTORY = dirname(fileURLToPath(import.meta.url));
const REPOSITORY_ROOT = resolve(SCRIPT_DIRECTORY, "..", "..");
const MANIFEST_PATH = "contracts/reference/python-oracle-tree.json";
const REFERENCE_EXACT_PATHS = new Set([
  "Dockerfile",
  "clients/headless/music_output.py",
  "clients/headless/requirements.txt",
]);
const TRANSITION_ONLY_PATHS = new Set([
  ".github/scripts/compare-runtime.sh",
  ".github/scripts/runtime-differential.mjs",
  ".github/scripts/voice-differential.mjs",
  ".github/scripts/voice-differential.test.mjs",
  "Dockerfile.rust",
  "Dockerfile.rust.dockerignore",
]);
const PYTHON_BASENAMES = new Set([
  ".python-version",
  "Pipfile",
  "Pipfile.lock",
  "pdm.lock",
  "poetry.lock",
  "pyproject.toml",
  "pytest.ini",
  "setup.cfg",
  "setup.py",
  "tox.ini",
  "uv.lock",
]);
const ACTIVE_REFERENCE_PATTERNS = [
  [/actions\/setup-python@/iu, "Python setup action"],
  [/package-ecosystem:\s*["']?pip/iu, "pip dependency ecosystem"],
  [/^\s*FROM\s+python(?::|\s)/imu, "Python container base"],
  [/(?:^|[\s"'`])(?:python3?|pip3?|uv)\s+(?:-m|install|sync|run|export|wheel|test|check)\b/imu, "Python package/runtime command"],
  [/backend\/(?:app|tests|pyproject\.toml|uv\.lock|\.venv)/iu, "removed backend path"],
  [/Dockerfile\.rust|music_output\.py|requirements\.txt/iu, "transition-only path"],
];

function usage() {
  return [
    "usage: node .github/scripts/rewrite-tree.mjs <mode>",
    "",
    "modes:",
    "  fingerprint  print the current frozen-oracle fingerprint",
    "  reference    verify the clean pre-cutover tree and frozen oracle",
    "  final        verify a clean post-acceptance Rust-only tree",
  ].join("\n");
}

function runGit(arguments_) {
  const result = spawnSync("git", arguments_, {
    cwd: REPOSITORY_ROOT,
    encoding: "utf8",
    maxBuffer: 32 * 1_024 * 1_024,
  });
  if (result.error) throw result.error;
  if (result.status !== 0) {
    throw new Error(`git ${arguments_.join(" ")} failed: ${String(result.stderr).trim()}`);
  }
  return String(result.stdout);
}

function assertCleanWorktree() {
  if (runGit(["status", "--porcelain=v1", "--untracked-files=all"]).trim()) {
    throw new Error("rewrite-tree verification requires a clean Git worktree");
  }
}

function trackedEntries() {
  const output = runGit(["ls-tree", "-r", "-z", "--full-tree", "HEAD"]);
  return output
    .split("\0")
    .filter(Boolean)
    .map((record) => {
      const separator = record.indexOf("\t");
      if (separator < 0) throw new Error("git ls-tree returned an invalid record");
      const [mode, type, oid] = record.slice(0, separator).split(" ");
      return { mode, type, oid, path: record.slice(separator + 1) };
    });
}

export function isReferenceArtifact(path) {
  return path.startsWith("backend/") || REFERENCE_EXACT_PATHS.has(path);
}

export function isPythonArtifact(path) {
  const name = basename(path);
  return path.toLowerCase().endsWith(".py")
    || path.toLowerCase().endsWith(".pyi")
    || /^requirements(?:[-_.].*)?\.txt$/iu.test(name)
    || PYTHON_BASENAMES.has(name);
}

export function referenceFingerprint(entries) {
  const selected = entries
    .filter((entry) => entry.type === "blob" && isReferenceArtifact(entry.path))
    .sort((left, right) => left.path < right.path ? -1 : left.path > right.path ? 1 : 0);
  const digest = createHash("sha256");
  for (const entry of selected) digest.update(`${entry.path}\0${entry.oid}\n`, "utf8");
  return {
    tracked_files: selected.length,
    git_blob_manifest_sha256: digest.digest("hex"),
  };
}

function isActiveSurface(path) {
  if (path === ".github/scripts/rewrite-tree.mjs" || path === ".github/scripts/rewrite-tree.test.mjs") {
    return false;
  }
  return path === "Dockerfile"
    || path === ".dockerignore"
    || path.startsWith(".github/")
    || path.startsWith("clients/")
    || (!path.includes("/") && /\.(?:md|toml|yml|yaml)$/iu.test(path));
}

function isGeneratedOrSensitiveArtifact(path) {
  if (/(^|\/)(?:target|node_modules|\.venv|__pycache__|fuzz\/artifacts|fuzz\/coverage)(\/|$)/iu.test(path)) {
    return true;
  }
  if (/(?:^|\/)\.env$/iu.test(path)) return true;
  return /\.(?:aac|db|flac|key|m4a|mp3|ogg|opus|p12|pem|pfx|profraw|pyc|pyo|sqlite|sqlite3|wav|wma)$/iu.test(path);
}

export function finalTreeViolations(entries, readText) {
  const paths = entries.filter((entry) => entry.type === "blob").map((entry) => entry.path);
  const violations = [];
  if (!paths.includes("Dockerfile")) violations.push("Dockerfile: final Rust image definition is missing");
  const backendPaths = paths.filter((path) => path.startsWith("backend/"));
  if (backendPaths.length > 0) {
    violations.push(`backend/: ${backendPaths.length} tracked frozen-oracle files remain`);
  }
  for (const path of paths) {
    if (path.startsWith("backend/")) continue;
    if (isPythonArtifact(path)) {
      violations.push(`${path}: project-owned Python artifact remains`);
      continue;
    }
    if (TRANSITION_ONLY_PATHS.has(path)) {
      violations.push(`${path}: transition-only rewrite artifact remains`);
      continue;
    }
    if (isGeneratedOrSensitiveArtifact(path)) {
      violations.push(`${path}: generated, media, database, or sensitive artifact is tracked`);
      continue;
    }
    if (!isActiveSurface(path)) continue;
    const content = readText(path);
    for (const [pattern, description] of ACTIVE_REFERENCE_PATTERNS) {
      if (pattern.test(content)) violations.push(`${path}: active ${description} remains`);
    }
  }
  return [...new Set(violations)].sort();
}

function checkReference(entries) {
  assertCleanWorktree();
  const paths = entries.filter((entry) => entry.type === "blob").map((entry) => entry.path);
  const unexpected = paths.filter((path) => isPythonArtifact(path) && !isReferenceArtifact(path));
  if (unexpected.length > 0) {
    throw new Error(`Python artifacts escaped the frozen reference boundary:\n${unexpected.join("\n")}`);
  }
  const manifest = JSON.parse(readFileSync(resolve(REPOSITORY_ROOT, MANIFEST_PATH), "utf8"));
  const actual = referenceFingerprint(entries);
  if (manifest.schema_version !== "python-oracle-tree/v1"
      || manifest.tracked_files !== actual.tracked_files
      || manifest.git_blob_manifest_sha256 !== actual.git_blob_manifest_sha256) {
    throw new Error([
      "frozen Python oracle drifted from its checked-in manifest",
      `expected ${manifest.tracked_files} files / ${manifest.git_blob_manifest_sha256}`,
      `actual   ${actual.tracked_files} files / ${actual.git_blob_manifest_sha256}`,
    ].join("\n"));
  }
  process.stdout.write(`frozen Python oracle is unchanged (${actual.tracked_files} tracked files)\n`);
}

function checkFinal(entries) {
  assertCleanWorktree();
  const violations = finalTreeViolations(
    entries,
    (path) => readFileSync(resolve(REPOSITORY_ROOT, path), "utf8"),
  );
  if (violations.length > 0) {
    throw new Error(`final Rust-only tree check failed:\n${violations.map((item) => `- ${item}`).join("\n")}`);
  }
  process.stdout.write("final tracked tree is Rust-only and contains no transition or generated artifacts\n");
}

function main() {
  const [mode, ...extra] = process.argv.slice(2);
  if (extra.length > 0 || !mode || mode === "--help" || mode === "-h") {
    if (!mode || mode === "--help" || mode === "-h") {
      process.stdout.write(`${usage()}\n`);
      return;
    }
    throw new Error(usage());
  }
  const entries = trackedEntries();
  if (mode === "fingerprint") {
    process.stdout.write(`${JSON.stringify(referenceFingerprint(entries), null, 2)}\n`);
  } else if (mode === "reference") {
    checkReference(entries);
  } else if (mode === "final") {
    checkFinal(entries);
  } else {
    throw new Error(usage());
  }
}

const invokedPath = process.argv[1] ? pathToFileURL(resolve(process.argv[1])).href : "";
if (invokedPath === import.meta.url) {
  try {
    main();
  } catch (error) {
    process.stderr.write(`rewrite-tree: ${error instanceof Error ? error.message : String(error)}\n`);
    process.exitCode = 1;
  }
}
