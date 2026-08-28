import assert from "node:assert/strict";
import test from "node:test";

import { architectureViolations, sourceStateViolations } from "./rust-architecture.mjs";

const ALLOWED_DEPENDENCIES = new Map([
  ["music-domain", []],
  ["music-application", ["music-domain"]],
  ["music-protocol", []],
  ["music-storage", ["music-application", "music-domain"]],
  ["music-media", ["music-application", "music-domain"]],
  ["music-analysis", ["music-application"]],
  ["music-server", [
    "music-analysis",
    "music-application",
    "music-domain",
    "music-media",
    "music-protocol",
    "music-storage",
  ]],
  ["music-output", ["music-protocol"]],
]);

function dependency(name, overrides = {}) {
  return {
    name,
    path: `/repo/crates/${name}`,
    rename: null,
    source: null,
    ...overrides,
  };
}

function metadata(overrides = {}) {
  const packages = [...ALLOWED_DEPENDENCIES].map(([name, dependencies]) => ({
    id: `path+file:///repo/crates/${name}#0.1.0`,
    name,
    manifest_path: `/repo/crates/${name}/Cargo.toml`,
    dependencies: dependencies.map((dependencyName) => dependency(dependencyName)),
  }));
  return {
    packages,
    workspace_members: packages.map((candidate) => candidate.id),
    ...overrides,
  };
}

test("accepted eight-crate dependency graph passes", () => {
  assert.deepEqual(architectureViolations(metadata()), []);
});

test("reverse dependencies and internal aliases fail", () => {
  const fixture = metadata();
  const domain = fixture.packages.find((candidate) => candidate.name === "music-domain");
  domain.dependencies.push(dependency("music-server", { rename: "runtime" }));
  assert.deepEqual(architectureViolations(fixture), [
    "music-domain aliases internal crate music-server as runtime",
    "music-domain has forbidden dependency on music-server",
  ]);
});

test("registry stand-ins and unapproved path crates fail", () => {
  const fixture = metadata();
  const application = fixture.packages.find((candidate) => candidate.name === "music-application");
  const output = fixture.packages.find((candidate) => candidate.name === "music-output");
  application.dependencies = [
    dependency("music-domain", { path: null, source: "registry+https://example.invalid/index" }),
    dependency("local-helper"),
  ];
  output.dependencies = [dependency("music-protocol", { path: "/repo/vendor/music-protocol" })];
  assert.deepEqual(architectureViolations(fixture), [
    "music-application does not resolve music-domain as a local path dependency",
    "music-application uses an unapproved path dependency: local-helper",
    "music-output resolves music-protocol from an unexpected path: /repo/vendor/music-protocol",
  ]);
});

test("workspace shape and crate locations are fixed architecture decisions", () => {
  const fixture = metadata();
  fixture.packages = fixture.packages.filter((candidate) => candidate.name !== "music-output");
  fixture.workspace_members = fixture.packages.map((candidate) => candidate.id);
  fixture.packages.push({
    id: "path+file:///repo/crates/music-extra#0.1.0",
    name: "music-extra",
    manifest_path: "/repo/elsewhere/music-extra/Cargo.toml",
    dependencies: [],
  });
  fixture.workspace_members.push(fixture.packages.at(-1).id);
  const domain = fixture.packages.find((candidate) => candidate.name === "music-domain");
  domain.manifest_path = "/repo/elsewhere/music-domain/Cargo.toml";
  assert.deepEqual(architectureViolations(fixture), [
    "music-domain has an unexpected manifest path: /repo/elsewhere/music-domain/Cargo.toml",
    "required workspace crate is missing: music-output",
    "unapproved workspace crate exists: music-extra",
  ]);
});

test("runtime module globals fail while the isolated fuzz caches remain explicit", () => {
  assert.deepEqual(sourceStateViolations([{
    path: "crates/music-server/src/runtime.rs",
    content: "static RUNTIME: OnceLock<AppRuntime> = OnceLock::new();\n",
  }]), [
    "crates/music-server/src/runtime.rs: module-global static RUNTIME is not approved; inject owned state through AppRuntime",
  ]);
  assert.deepEqual(sourceStateViolations([{
    path: "crates/music-application/src/assistant/fuzzing.rs",
    content: [
      "static EQ_TASK: OnceLock<Option<EqDraftTask>> = OnceLock::new();",
      "static PLAYLIST_TASK: OnceLock<Option<ModelPlaylistTask>> = OnceLock::new();",
      "static TAGGER_TASK: OnceLock<Option<ModelTaggerBatch>> = OnceLock::new();",
      "static VOCABULARY: OnceLock<Option<super::TagVocabularySnapshot>> = OnceLock::new();",
    ].join("\n"),
  }]), []);
  assert.deepEqual(sourceStateViolations([{
    path: "crates/music-application/src/assistant/fuzzing.rs",
    content: "static EQ_TASK: Mutex<Option<EqDraftTask>> = Mutex::new(None);\n",
  }]), [
    "crates/music-application/src/assistant/fuzzing.rs: module-global static EQ_TASK is not approved; inject owned state through AppRuntime",
  ]);
});
