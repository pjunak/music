import assert from "node:assert/strict";
import test from "node:test";

import {
  finalTreeViolations,
  isPythonArtifact,
  isReferenceArtifact,
  referenceFingerprint,
} from "./rewrite-tree.mjs";

function entry(path, oid = "a".repeat(40)) {
  return { mode: "100644", type: "blob", oid, path };
}

test("oracle fingerprint is ordered, content-sensitive, and boundary-scoped", () => {
  const first = referenceFingerprint([
    entry("unrelated.txt", "f".repeat(40)),
    entry("backend/app/main.py", "a".repeat(40)),
    entry("Dockerfile", "b".repeat(40)),
  ]);
  const reordered = referenceFingerprint([
    entry("Dockerfile", "b".repeat(40)),
    entry("backend/app/main.py", "a".repeat(40)),
  ]);
  const changed = referenceFingerprint([
    entry("Dockerfile", "c".repeat(40)),
    entry("backend/app/main.py", "a".repeat(40)),
  ]);
  assert.deepEqual(first, reordered);
  assert.notEqual(first.git_blob_manifest_sha256, changed.git_blob_manifest_sha256);
  assert.equal(first.tracked_files, 2);
});

test("temporary Python artifacts are confined to the explicit oracle boundary", () => {
  assert.equal(isReferenceArtifact("backend/app/main.py"), true);
  assert.equal(isReferenceArtifact("clients/headless/music_output.py"), true);
  assert.equal(isReferenceArtifact("tools/helper.py"), false);
  assert.equal(isPythonArtifact("backend/pyproject.toml"), true);
  assert.equal(isPythonArtifact("tools/requirements-dev.txt"), true);
  assert.equal(isPythonArtifact("docs/ADR-013-python.md"), false);
});

test("final scan rejects runtime remnants, transition tools, and generated artifacts", () => {
  const contents = new Map([
    ["Dockerfile", "FROM python:3.14-slim\n"],
    ["README.md", "Build with Dockerfile.rust and uv sync.\n"],
    [".github/workflows/build.yml", "uses: actions/setup-python@v7\n"],
  ]);
  const violations = finalTreeViolations([
    entry("Dockerfile"),
    entry("Dockerfile.rust"),
    entry("README.md"),
    entry("backend/app/main.py"),
    entry("tools/requirements-dev.txt"),
    entry(".github/scripts/voice-differential.mjs"),
    entry(".github/workflows/build.yml"),
    entry("fixtures/private.flac"),
  ], (path) => contents.get(path) ?? "");
  const report = violations.join("\n");
  assert.match(report, /backend|Python artifact|transition-only|generated|container base|setup action/iu);
  assert.equal(report.match(/backend\//gu)?.length, 1);
});

test("final scan permits compatibility history and a native active surface", () => {
  const contents = new Map([
    ["Dockerfile", "FROM debian:stable-slim\nCOPY music-server /usr/local/bin/music-server\n"],
    ["README.md", "The release image contains no Python runtime.\n"],
    ["AGENTS.md", "Keep legacy data fixtures compatible.\n"],
  ]);
  const violations = finalTreeViolations([
    entry("Dockerfile"),
    entry("README.md"),
    entry("AGENTS.md"),
    entry("docs/ADR-013-python-history.md"),
    entry("crates/music-storage/src/schema.rs"),
    entry("contracts/reference/python-oracle-tree.json"),
  ], (path) => contents.get(path) ?? "");
  assert.deepEqual(violations, []);
});
