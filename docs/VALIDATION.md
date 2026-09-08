# Validation and development

Run commands from the repository root unless a different directory is shown.
Read the relevant rows before choosing checks. CI/release coverage is unchanged.

| Change | Final checks |
|---|---|
| Prose or agent guidance only | Review the diff and local links; verify changed command/contract claims against their owner. Run contract documentation checks when inventory values or generated contracts change. No runtime rebuild solely for prose. |
| Rust behavior | Focused regression tests while iterating; formatting, architecture checks, workspace check/Clippy/nextest/doc tests and contracts check below on the final change. |
| Frontend behavior | Frontend lint, typecheck, tests and build; inspect affected visible flows. Rust gates apply when a server/wire contract also changes. |
| HTTP/WS, schema, persistence, auth or shared playback | Affected Rust/frontend gates plus relevant client serialization, reconnect, failure and compatibility tests; coordinate Baton when its wire behavior changes. |
| Dependencies, licenses or toolchains | Affected runtime gates plus deny/audit/machete for each changed dependency graph; preserve separate fuzz lockfile coverage. |
| Fuzz sources/configuration | Fuzz formatting, Clippy and applicable dependency checks below. |
| Packaging, release image or runtime-language boundary | Full applicable gates, final-tree checks, headless release binary and image verification on a Docker host. |

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
node --test .github/scripts/rust-architecture.test.mjs
node --test tools/mood-pilot.test.mjs
node .github/scripts/rust-architecture.mjs
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
node --test .github/scripts/rewrite-tree.test.mjs
node .github/scripts/rewrite-tree.mjs final
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

The frontend uses the native TypeScript 7 compiler and Oxlint. The local
`local/stable-store-selector` rule is an Oxlint JS plugin under
`frontend/lint-rules/`; keep its real-binary fixture test when changing it.

## Local development

Run locally:

```powershell
# repository root
cargo run --locked -p music-server --bin music-server

# frontend/
npm run dev
```
