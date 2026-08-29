import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import test from "node:test";

import { finalTreeViolations, isPythonArtifact } from "./rewrite-tree.mjs";

function entry(path, oid = "a".repeat(40)) {
  return { mode: "100644", type: "blob", oid, path };
}

test("project-owned Python artifacts are detected by path and packaging name", () => {
  assert.equal(isPythonArtifact("legacy/main.py"), true);
  assert.equal(isPythonArtifact("tools/helper.pyi"), true);
  assert.equal(isPythonArtifact("tools/requirements-dev.txt"), true);
  assert.equal(isPythonArtifact("service/pyproject.toml"), true);
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
    entry("Dockerfile.rust.dockerignore"),
    entry("README.md"),
    entry("backend/app/main.py"),
    entry("tools/requirements-dev.txt"),
    entry(".github/scripts/voice-differential.mjs"),
    entry(".github/workflows/build.yml"),
    entry("fixtures/private.flac"),
  ], (path) => contents.get(path) ?? "");
  const report = violations.join("\n");
  assert.match(report, /backend|Python artifact|transition-only|generated|container base|setup action/iu);
  assert.match(report, /Dockerfile\.rust\.dockerignore: transition-only/iu);
  assert.equal(report.match(/backend\//gu)?.length, 1);
});

test("final scan permits compatibility history and a native active surface", () => {
  const contents = new Map([
    ["Dockerfile", "FROM debian:stable-slim\nCOPY music-server /usr/local/bin/music-server\n"],
    ["README.md", "The release image contains no Python runtime.\n"],
    ["AGENTS.md", "Keep legacy data fixtures compatible.\n"],
    [
      ".github/scripts/verify-rust-image.sh",
      readFileSync(new URL("./verify-rust-image.sh", import.meta.url), "utf8"),
    ],
  ]);
  const violations = finalTreeViolations([
    entry("Dockerfile"),
    entry("README.md"),
    entry("AGENTS.md"),
    entry(".github/scripts/verify-rust-image.sh"),
    entry("docs/ADR-013-python-history.md"),
    entry("crates/music-storage/src/schema.rs"),
  ], (path) => contents.get(path) ?? "");
  assert.deepEqual(violations, []);
});
