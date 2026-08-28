import { spawnSync } from "node:child_process";
import { readFileSync, readdirSync } from "node:fs";
import { dirname, relative, resolve } from "node:path";
import { fileURLToPath, pathToFileURL } from "node:url";

const SCRIPT_DIRECTORY = dirname(fileURLToPath(import.meta.url));
const REPOSITORY_ROOT = resolve(SCRIPT_DIRECTORY, "..", "..");

const CRATE_RULES = new Map([
  ["music-domain", { allowed: [], manifest: "crates/music-domain/Cargo.toml" }],
  ["music-application", {
    allowed: ["music-domain"],
    manifest: "crates/music-application/Cargo.toml",
  }],
  ["music-protocol", { allowed: [], manifest: "crates/music-protocol/Cargo.toml" }],
  ["music-storage", {
    allowed: ["music-application", "music-domain"],
    manifest: "crates/music-storage/Cargo.toml",
  }],
  ["music-media", {
    allowed: ["music-application", "music-domain"],
    manifest: "crates/music-media/Cargo.toml",
  }],
  ["music-analysis", {
    allowed: ["music-application", "music-domain"],
    manifest: "crates/music-analysis/Cargo.toml",
  }],
  ["music-server", {
    allowed: [
      "music-analysis",
      "music-application",
      "music-domain",
      "music-media",
      "music-protocol",
      "music-storage",
    ],
    manifest: "crates/music-server/Cargo.toml",
  }],
  ["music-output", {
    allowed: ["music-protocol"],
    manifest: "crates/music-output/Cargo.toml",
  }],
]);
const FUZZ_STATIC_ALLOWLIST = new Map([
  ["crates/music-application/src/assistant/fuzzing.rs:EQ_TASK", "OnceLock<Option<EqDraftTask>>"],
  ["crates/music-application/src/assistant/fuzzing.rs:PLAYLIST_TASK", "OnceLock<Option<ModelPlaylistTask>>"],
  ["crates/music-application/src/assistant/fuzzing.rs:TAGGER_TASK", "OnceLock<Option<ModelTaggerBatch>>"],
  ["crates/music-application/src/assistant/fuzzing.rs:VOCABULARY", "OnceLock<Option<super::TagVocabularySnapshot>>"],
]);
const APPROVED_TOKIO_SPAWN_COUNTS = new Map([
  ["crates/music-application/src/jobs.rs", 2],
  ["crates/music-application/src/library.rs", 1],
  ["crates/music-application/src/modes.rs", 1],
  ["crates/music-application/src/playback/actor.rs", 1],
  ["crates/music-server/src/supervisor.rs", 1],
  ["crates/music-server/src/websocket.rs", 1],
]);
const APPROVED_SPAWN_BLOCKING_COUNTS = new Map([
  ["crates/music-application/src/auth.rs", 1],
  ["crates/music-media/src/discovery.rs", 1],
  ["crates/music-media/src/mode_mutation.rs", 5],
  ["crates/music-media/src/modes.rs", 1],
  ["crates/music-media/src/mutation.rs", 4],
  ["crates/music-media/src/sfx.rs", 8],
  ["crates/music-server/src/admin.rs", 4],
  ["crates/music-server/src/bin/music-cli.rs", 1],
  ["crates/music-server/src/blocking.rs", 1],
  ["crates/music-storage/src/backup.rs", 1],
  ["crates/music-storage/src/devices.rs", 3],
  ["crates/music-storage/src/migration.rs", 2],
]);

function portablePath(path) {
  return String(path).replaceAll("\\", "/").replace(/\/+$/u, "");
}

function workspacePackages(metadata, violations) {
  if (!metadata || !Array.isArray(metadata.packages) || !Array.isArray(metadata.workspace_members)) {
    violations.push("cargo metadata did not contain packages and workspace_members arrays");
    return [];
  }
  const memberIds = new Set(metadata.workspace_members);
  return metadata.packages.filter((candidate) => memberIds.has(candidate.id));
}

export function architectureViolations(metadata) {
  const violations = [];
  const packages = workspacePackages(metadata, violations);
  const packagesByName = new Map();

  for (const candidate of packages) {
    if (!candidate || typeof candidate.name !== "string") {
      violations.push("workspace metadata contains a package without a name");
      continue;
    }
    if (packagesByName.has(candidate.name)) {
      violations.push(`workspace package name is duplicated: ${candidate.name}`);
      continue;
    }
    packagesByName.set(candidate.name, candidate);
  }

  for (const name of CRATE_RULES.keys()) {
    if (!packagesByName.has(name)) violations.push(`required workspace crate is missing: ${name}`);
  }
  for (const name of packagesByName.keys()) {
    if (!CRATE_RULES.has(name)) violations.push(`unapproved workspace crate exists: ${name}`);
  }

  for (const [name, rule] of CRATE_RULES) {
    const candidate = packagesByName.get(name);
    if (!candidate) continue;
    const manifestPath = portablePath(candidate.manifest_path);
    if (!manifestPath.endsWith(`/${rule.manifest}`) && manifestPath !== rule.manifest) {
      violations.push(`${name} has an unexpected manifest path: ${manifestPath}`);
    }

    const dependencies = Array.isArray(candidate.dependencies) ? candidate.dependencies : [];
    for (const dependency of dependencies) {
      const dependencyName = dependency?.name;
      if (typeof dependencyName !== "string") continue;
      const isKnownInternal = CRATE_RULES.has(dependencyName);
      const isPathDependency = typeof dependency.path === "string";

      if (isPathDependency && !isKnownInternal) {
        violations.push(`${name} uses an unapproved path dependency: ${dependencyName}`);
        continue;
      }
      if (!isKnownInternal) continue;
      if (!isPathDependency || dependency.source !== null) {
        violations.push(`${name} does not resolve ${dependencyName} as a local path dependency`);
      } else {
        const dependencyRoot = CRATE_RULES.get(dependencyName).manifest.replace(/\/Cargo\.toml$/u, "");
        const dependencyPath = portablePath(dependency.path);
        if (!dependencyPath.endsWith(`/${dependencyRoot}`) && dependencyPath !== dependencyRoot) {
          violations.push(`${name} resolves ${dependencyName} from an unexpected path: ${dependencyPath}`);
        }
      }
      if (dependency.rename !== null && dependency.rename !== undefined) {
        violations.push(`${name} aliases internal crate ${dependencyName} as ${dependency.rename}`);
      }
      if (!rule.allowed.includes(dependencyName)) {
        violations.push(`${name} has forbidden dependency on ${dependencyName}`);
      }
    }
  }

  return [...new Set(violations)].sort();
}

export function sourceStateViolations(files) {
  const violations = [];
  const declaration = /^\s*(?:pub(?:\s*\([^\r\n)]*\))?\s+)?static\s+(?:mut\s+)?([A-Za-z_][A-Za-z0-9_]*)\s*:\s*([^=;]+?)(?:=|;)/gmu;
  for (const file of files) {
    const path = portablePath(file.path);
    for (const match of String(file.content).matchAll(declaration)) {
      const name = match[1];
      const expectedType = FUZZ_STATIC_ALLOWLIST.get(`${path}:${name}`);
      const actualType = match[2].replace(/\s+/gu, "");
      if (!expectedType || actualType !== expectedType.replace(/\s+/gu, "")) {
        violations.push(`${path}: module-global static ${name} is not approved; inject owned state through AppRuntime`);
      }
    }
  }
  return [...new Set(violations)].sort();
}

function productionSource(content) {
  const source = String(content);
  const testModule = /^\s*#\[cfg\(test\)\]\s*\r?\n\s*mod\s+tests\s*\{/mu.exec(source);
  return testModule ? source.slice(0, testModule.index) : source;
}

function occurrenceCount(source, pattern) {
  return [...source.matchAll(pattern)].length;
}

export function sourceConcurrencyViolations(files) {
  const violations = [];
  const unboundedChannel = /\bmpsc::unbounded_channel\s*\(|\bmpsc::channel\s*\(\s*\)|\b(?:async_channel|crossbeam_channel|flume)::unbounded\s*\(/gu;
  for (const file of files) {
    const path = portablePath(file.path);
    const source = productionSource(file.content);
    if (occurrenceCount(source, unboundedChannel) > 0) {
      violations.push(`${path}: unbounded production channel is forbidden`);
    }

    const tokioSpawns = occurrenceCount(source, /\btokio::spawn\s*\(/gu);
    const approvedTokioSpawns = APPROVED_TOKIO_SPAWN_COUNTS.get(path) ?? 0;
    if (tokioSpawns !== approvedTokioSpawns) {
      violations.push(
        `${path}: production tokio::spawn count changed (approved ${approvedTokioSpawns}, observed ${tokioSpawns}); review ownership and supervision`,
      );
    }

    const blockingSpawns = occurrenceCount(source, /\btokio::task::spawn_blocking\s*\(/gu);
    const approvedBlockingSpawns = APPROVED_SPAWN_BLOCKING_COUNTS.get(path) ?? 0;
    if (blockingSpawns !== approvedBlockingSpawns) {
      violations.push(
        `${path}: production spawn_blocking count changed (approved ${approvedBlockingSpawns}, observed ${blockingSpawns}); review bounded admission`,
      );
    }
  }
  return [...new Set(violations)].sort();
}

function rustSourceFiles(directory = resolve(REPOSITORY_ROOT, "crates")) {
  const files = [];
  for (const entry of readdirSync(directory, { withFileTypes: true })) {
    const path = resolve(directory, entry.name);
    if (entry.isDirectory()) files.push(...rustSourceFiles(path));
    else if (entry.isFile() && entry.name.endsWith(".rs")) {
      files.push({
        path: portablePath(relative(REPOSITORY_ROOT, path)),
        content: readFileSync(path, "utf8"),
      });
    }
  }
  return files;
}

function usage() {
  return [
    "usage: node .github/scripts/rust-architecture.mjs [--cargo <path>] [--toolchain <name>]",
    "",
    "Checks the locked Cargo metadata and source against the accepted architecture boundaries.",
  ].join("\n");
}

function parseArguments(arguments_) {
  const options = { cargo: "cargo", toolchain: null };
  for (let index = 0; index < arguments_.length; index += 1) {
    const argument = arguments_[index];
    if (argument === "--help" || argument === "-h") return null;
    if (argument !== "--cargo" && argument !== "--toolchain") throw new Error(usage());
    const value = arguments_[index + 1];
    if (!value || value.startsWith("--")) throw new Error(usage());
    if (argument === "--cargo") options.cargo = value;
    else options.toolchain = value.replace(/^\+/u, "");
    index += 1;
  }
  return options;
}

function loadMetadata(options) {
  const arguments_ = [];
  if (options.toolchain) arguments_.push(`+${options.toolchain}`);
  arguments_.push("metadata", "--locked", "--no-deps", "--format-version", "1");
  const result = spawnSync(options.cargo, arguments_, {
    cwd: REPOSITORY_ROOT,
    encoding: "utf8",
    maxBuffer: 32 * 1_024 * 1_024,
  });
  if (result.error) throw result.error;
  if (result.status !== 0) {
    const detail = String(result.stderr || result.stdout).trim();
    throw new Error(`cargo metadata failed${detail ? `: ${detail}` : ""}`);
  }
  try {
    return JSON.parse(String(result.stdout));
  } catch (error) {
    throw new Error("cargo metadata returned invalid JSON", { cause: error });
  }
}

function main() {
  const options = parseArguments(process.argv.slice(2));
  if (!options) {
    process.stdout.write(`${usage()}\n`);
    return;
  }
  const sourceFiles = rustSourceFiles();
  const violations = [
    ...architectureViolations(loadMetadata(options)),
    ...sourceStateViolations(sourceFiles),
    ...sourceConcurrencyViolations(sourceFiles),
  ].sort();
  if (violations.length > 0) {
    throw new Error(`Rust architecture check failed:\n${violations.map((item) => `- ${item}`).join("\n")}`);
  }
  process.stdout.write(`Rust architecture boundaries verified (${CRATE_RULES.size} workspace crates)\n`);
}

const invokedPath = process.argv[1] ? pathToFileURL(resolve(process.argv[1])).href : "";
if (invokedPath === import.meta.url) {
  try {
    main();
  } catch (error) {
    process.stderr.write(`rust-architecture: ${error instanceof Error ? error.message : String(error)}\n`);
    process.exitCode = 1;
  }
}
