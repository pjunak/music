# Validation and development

Run commands from the repository root unless a different directory is shown.
Read the relevant rows before choosing checks. CI/release coverage is unchanged.

| Change | Final checks |
|---|---|
| Prose or agent guidance only | Review the diff and local links; verify changed command/contract claims against their owner. Run contract documentation checks when inventory values or generated contracts change. No runtime rebuild solely for prose. |
| Rust behavior | Focused regression tests while iterating; formatting, architecture checks, workspace check/Clippy/nextest/doc tests and contracts check below on the final change. |
| Frontend behavior | Frontend lint, typecheck, tests and build; inspect affected visible flows. Rust gates apply when a server/wire contract also changes. |
| Development audio references | `node --test tools/effnet-reference.test.mts tools/effnet-stream.test.mts tools/jev-audio-evidence.test.mts`; verify existing pinned fixtures when sharing their loader. Licensed-model/private-audio comparisons remain explicit, separately recorded experiments. |
| Offline mood pilot | `node --test tools/mood-pilot.test.mts`; affected frontend gates for exported files and controls. Exercise changed CLI operations, including prediction-free readiness and export handling. Private listening judgments remain separate acceptance evidence. |
| Jev experiment tooling | `cargo test --locked -p music-server --example jev-compare`; `node --test tools/jev-pilot-report.test.mts`; inspect offline plans. Paid runs require an explicit reviewed plan and budget, never an automatic test or retry. |
| Graded Jev listening experiment | Above Jev checks plus `node --test tools/jev-corpus-prepare.test.mts tools/jev-graded-report.test.mts`, tool typecheck/lint, immutable-plan and explicit source-hash review. See [the experiment design](JEV_GRADED_TAGGING_PILOT.md). Never convert confidence into musical relevance or incomplete results into zero. |
| Short-question Jev comparison | Native Jev example tests plus `node --test tools/jev-simple-report.test.mts tools/jev-graded-report.test.mts`, tool typecheck/lint, exact baseline pairing and source-hash review. See [the comparison design](JEV_SIMPLE_QUESTION_PILOT.md). Noul probabilities and ordinal Score positions have different meanings. |
| Jev evidence ablation | Native Jev example tests and Rust gates; inspect the frozen baseline/auxiliary hashes, exact request and input-unit bounds, factor isolation, source hashes, repeats and missing-evidence controls. Export only a complete journal through `ablation-report`. See [the investigation](JEV_EVIDENCE_RESEARCH.md). Raw answers and model costs do not establish listening accuracy. |
| Jev algorithmic feature experiments | Native Jev example tests and Rust gates; validate controlled signal behavior, extractor/model contracts, exact original hashes and duration, family isolation, interval precision, immutable reservations and repeats. Export only a complete journal through `feature-report`. See [the feature study](JEV_ALGORITHMIC_FEATURE_PILOT.md). Affect estimates remain learned clues; additional input or stronger answers do not establish accuracy. |
| Targeted Jev feature experiments | Native Jev example tests and Rust gates; inspect source/duration binding, controlled pulse/roughness behavior, exact profile labels and temporal coverage, pooled arithmetic, representation isolation, missingness and immutable budgets. Export only a complete journal through `targeted-report`. See [the targeted study](JEV_TARGETED_FEATURE_PILOT.md). Beat/mode estimates and classifier responses are fallible evidence, not verified mood tags. |
| HTTP/WS, schema, persistence, auth or shared playback | Affected Rust/frontend gates plus relevant client serialization, reconnect, failure and compatibility tests; coordinate Baton when its wire behavior changes. |
| Dependencies, licenses or toolchains | Affected runtime gates plus deny/audit/machete for each changed dependency graph; preserve separate fuzz lockfile coverage. |
| Fuzz sources/configuration | Fuzz formatting, Clippy and applicable dependency checks below. |
| Packaging, release image or runtime-language boundary | Full applicable gates, final-tree checks, headless release binary and image verification on a Docker host. |

Workflow changes also run `node --test .github/scripts/workflow-policy.test.mts`.
It covers pull requests, direct manual verification and reusable release calls.
The release caller disables the reusable image job because publication already
builds and smoke-tests the image; GitHub retains the caller's event name inside
the reusable workflow. Both Cargo workspaces (`.` and `fuzz`) are cached, and
all compile/test commands use the checked-in lockfiles. Vitest companion
dependency updates are grouped to preserve their matching peer versions.

The separate fuzz workspace links the application crates, including `music-server`.
Adding a dependency to one of those crates can require updates to both lockfiles even
when the package already exists transitively and no version changes. For dependency
edits, check both workspaces before longer builds with `cargo metadata --locked
--format-version 1` and `cargo metadata --manifest-path fuzz/Cargo.toml --locked
--format-version 1`. Include the fuzz Clippy and dependency checks below in validation.

[Dependabot](../.github/dependabot.yml) covers both Cargo directories in one entry.
The Utoipa family is grouped first so `utoipa` and `utoipa-axum` stay compatible;
remaining dependencies use `group-by: dependency-name` to update each dependency
across both lockfiles in one pull request. Major Utoipa upgrades also require
inspection of generated OpenAPI changes: compilation alone does not establish
schema compatibility. Keep that migration separate from unrelated patch updates.

Use current fixtures and local service instances. Hardware/private-library and
paid-provider evidence stays separately identified; a documentation edit or
passing local suite does not authorize an external run. Probe available tools
before reporting an environment limitation. Do not repeat successful checks
on unchanged inputs without a new failure or unresolved concern.

No dependency installation is needed for prose-only work. If a command is
unavailable, check the current environment before following the setup below.

## Rust command inventory

From the repository root:

```powershell
cargo fmt --all --check
node --test .github/scripts/rust-architecture.test.mts
node --test tools/mood-pilot.test.mts
node --test tools/cleanup-pilot.test.mts
node --test tools/effnet-reference.test.mts tools/effnet-stream.test.mts
node .github/scripts/rust-architecture.mts
cargo check --workspace --all-targets
cargo clippy --workspace --all-targets --all-features -- -D warnings
cargo nextest run --workspace --all-features
cargo test --workspace --all-features --doc
cargo build --locked --release -p music-output --bin music-output
cargo run --locked -p music-server --bin music-cli -- contracts check --root .
cargo deny check
cargo audit --deny warnings
cargo fmt --manifest-path fuzz/Cargo.toml --all -- --check
cargo clippy --manifest-path fuzz/Cargo.toml --bins --locked -- -D warnings
cargo deny --manifest-path fuzz/Cargo.toml --locked check
cargo audit --file fuzz/Cargo.lock --deny warnings
cargo machete .
cargo machete fuzz
node --test .github/scripts/rewrite-tree.test.mts
node .github/scripts/rewrite-tree.mts final
# After building the release image on a Docker host:
bash .github/scripts/verify-rust-image.sh music:test
```

The checked-in toolchain is authoritative. On Windows hosts without the Visual Studio C++ build
tools, install rustup's official GNU host and add
`+1.97.1-x86_64-pc-windows-gnu` immediately after `cargo` in the commands above. Linux CI and
release builds use the normal host selected by `rust-toolchain.toml`.
The architecture command also accepts `--cargo <rustup-cargo-path> --toolchain
1.97.1-x86_64-pc-windows-gnu` when Cargo is not on the Windows `PATH`.
The Rust gates are verified with `cargo-nextest` 0.9.143, `cargo-deny` 0.20.2, `cargo-audit`
0.22.2, and `cargo-machete` 0.9.2; install those exact versions with
`cargo install <tool> --version <version> --locked` only when they are unavailable or the required version changed.

For an operator acceptance pass on original audio, the optional
[audio probes](AUDIO_ANALYSIS_ACCEPTANCE.md) exercise factual coverage and voice inference,
repeat runs, cancellation and worker lifecycle measurements without touching a library.
Voice requires the separately supplied pinned model. Both v3 reports optionally read an
explicit cgroup v2 directory; fixture tests cover parsing, unknown values and read-only
scope without requiring Linux. Tests for both probes are included in the normal Rust
workspace gates; live cgroup, private-corpus and concurrent-playback measurements remain
separately reported.

The [MusiCNN reference fixtures](../crates/music-analysis/tests/fixtures/README.md) run
in the normal Rust suite without external packages or model weights. Regenerate them
with the pinned developer-only reference when changing the frontend; verify the fixed
numerical tolerance before accepting new fixtures. Voice decoder regressions also run when
FFmpeg is available on PATH or explicitly set by `MUSIC_TEST_FFMPEG`; invalid explicit paths
fail. Analytic stereo spectral and out-of-band suppression regressions use that same
real decoder path; they do not require reference packages or certify full upstream
resampler parity. The optional graph/worker tests require the separately supplied checksum-pinned
licensed model in `MUSIC_TEST_VOICE_MODEL` and, for the worker, `MUSIC_TEST_FFMPEG`.
The [dated reference notes](../crates/music-analysis/tests/fixtures/README.md#voice-decoding-and-ending-acceptance-25-september-2026)
record which tests actually exercised those tools and their remaining acceptance limits.

## Offline Jev investigation plans

The developer-only [example](../crates/music-server/examples/jev-compare.rs) uses
built-in synthetic cases by default. Its explicit `pilot-*` commands accept a
private file selection through the [assisted pilot](MOOD_PILOT.md#developer-jev-input-comparison).
No command loads application configuration or app credentials:

```powershell
cargo run --locked -p music-server --example jev-compare -- plan target/jev-framing-plan.json
cargo run --locked -p music-server --example jev-compare -- quality-plan target/jev-quality-plan.json
cargo run --locked -p music-server --example jev-compare -- evidence-plan target/jev-evidence-plan.json
# After a separately authorized comparison has produced this journal:
cargo run --locked -p music-server --example jev-compare -- report target/jev-framing-run.jsonl target/jev-framing-report.json
# After a separately authorized evidence experiment:
cargo run --locked -p music-server --example jev-compare -- evidence-report target/jev-evidence-run.jsonl target/jev-evidence-report.json
```

These commands are offline and refuse to overwrite their output. Comparison plans
account for every case in the current synthetic suite, including previously passing
positives, failures, custom definitions and safety controls. The baseline is the
exact production assessment plan. Its paired candidate removes only optional Noul
yes/no criteria from initial fit questions; instructions, state, definitions,
scopes, thresholds, Choice questions and partitioning stay identical. Cases with
no eligible evidence are recorded without requests. This replaces the rejected
dimension-specific experiment; old journals remain historical evidence.

The offline report requires a journal matching the current comparison plan. It
separates required-tag gains/losses, forbidden-tag threshold crossings, and changes
to tags with no explicit expectation. Missing or unpaired responses stay unknown;
partial runs cannot look complete. Duplicate, uncheckpointed, malformed or
wrong-model responses are rejected. Reported token totals include reporting counts
so absent usage cannot be mistaken for complete accounting. First-pass candidates
are not final tags: capacity, grounding, period applicability, conformance and
safety repeats are not evaluated by this experiment. No diagnostic pass rate is
reported and no production adoption follows automatically. Full plans include conformance,
the entire current quality suite, safety repeats, inference identity, scoring
expectations and conservative total request/input bounds.

The evidence plan holds production question bodies, criteria, definitions and
thresholds fixed while varying supplied data. It includes all existing cases and
vocabulary entries, previous successes, explicit negatives, custom definitions and
ending-conflict controls. Partitions are shared across arms and sized for the
largest state; they may differ from normal production packing. Identical/no-op
and empty-observation variants make no call and are explicitly reported. Six cases
have one budgeted identical-input repeat to expose score variation. Arm order
rotates deterministically; a repeat is not an automatic retry. All other identical
provider bodies share one physical request across cases/arms. Logical scoring
contexts retain their own expectations, and shared request IDs expose reuse and
differing expectation sets. Physical request labels describe only the first use.

Evidence reports compare each variant with its paired baseline within source cohort
and tag group. They retain raw scores, required-tag retention, forbidden candidates,
unscored crossings, separate period Choice snapshots, byte sizes, actual usage and
repeat controls. Period applicability and grounding are not run. Different arm
denominators cannot be ranked as comparable pass rates. Removing the only supporting
fact can correctly cause abstention; copied genre text under catalog attribution
does not establish the value of independently retrieved catalog facts. Empty arms,
incomplete pairs and absent usage remain distinct. No automatic best variant or
production adoption follows. The report shares the durable journal validator with
the question comparison; the larger evidence journal has a 256 MiB read bound.
Per-arm counts and paired usage describe logical comparisons, with separate distinct
request sets; shared inputs are not independent samples. Only root usage counts
each paid response once. Do not sum paired baseline or variant usage as total spend.

After separate paid-run authorization, `run`, `evidence-run` or `quality-run` requires
`--key-file` (a temporary secret outside source control), `--plan-sha256`,
`--max-requests` and `--max-input-units`, plus a new `--output` journal or
`--output-directory`. Use `--help` for the exact arguments. All runners require
the plan's exact total request/input bounds before reading credentials. The full
runner uses an isolated SQLite job database and
the production planner/executor/scorer. A
successfully completed job can still have `evaluation.passed: false`; neither
command writes application acceptance. Stop after any uncertain request and
inspect the saved journal; existing runs are never resumed or retried. See the
[dated live results](AI_ACCEPTANCE.md#jev-full-v8-validation-and-rollback-2026-09-28).

## Frontend

From `frontend/` (Node 26+). Run `npm ci` only for a missing or stale dependency
installation, including lockfile changes:

```powershell
npm ci
npm run lint
npm run typecheck
npm run test
npm run build
```

All maintained browser, test, and Node tooling source is TypeScript. Node 26 runs
the `.mts` tools directly. `npm run typecheck` checks the frontend and the root
`tsconfig.tools.json`; `npm run lint` first rejects tracked or newly added
JavaScript source, then runs repository-wide, type-aware Oxlint. Generated browser
JavaScript belongs in ignored build output. Do not add unchecked JavaScript helpers
or bypass these gates with `@ts-nocheck`.

The root `.oxlintrc.json` owns the common correctness and promise rules, separate
browser/Node environments, React compiler checks, and accessibility checks. The
TypeScript `local/stable-store-selector` plugin lives under `frontend/lint-rules/`.
Its real-binary tests and configuration probes verify the source directories,
type-aware promise checks, React purity/dependencies, and unused suppressions.
Keep these tests when changing lint configuration or plugin loading.

Intentional exceptions are narrow: compatibility sources retain ES5 `var` and
function syntax; compiler `set-state-in-effect` remains off for editor state that
mirrors canonical external state; autofocus is permitted for dialogs; semantic
ARIA status roles need not be replaced with form output elements; instrumental
audio does not require caption tracks. Tests may reference mock methods without
binding `this`, and only the three compatibility behavior suites may execute
generated local scripts using `Function`. Inline accessibility exceptions explain
dialog event bubbling and native draggable list rows at the affected elements.

The old-TV player and boot watchdog are authored in `frontend/compat/`. The Vite
plugin strips their types and parses both outputs as ES5 before serving/emitting
`/compat-mode.js` and inserting the inline watchdog. `tsconfig.compat.json` checks
their types against ES5/DOM APIs; TypeScript 7's minimum supported target is ES2015,
so the Acorn parse is the enforced runtime syntax boundary. Behavior tests use
the same generator as development and production. A frontend-only Docker build
checks frontend projects locally; repository tools are checked by CI before release.

## Local development

Run locally:

```powershell
# repository root
cargo run --locked -p music-server --bin music-server

# frontend/
npm run dev
```
