# Rust parser fuzzing

This independent Cargo workspace drives the production parsing and validation
surfaces that accept the least-trusted input:

- WebSocket client actions and server projections;
- portable media paths and rooted filesystem resolution;
- bounded mode, soundboard, cue, and preset YAML;
- authoring-import request documents; and
- all four strict Assistant structured-output validators.

`cargo-fuzz` requires a Unix-like host and nightly Rust. From the repository
root, build every target with:

```sh
cargo +nightly fuzz build
```

Run one target with a bounded input and per-case timeout, for example:

```sh
cargo +nightly fuzz run protocol_json -- -max_len=65536 -timeout=10
```

Crash artifacts are local and ignored. Minimized regression cases belong in
the owning crate's deterministic test suite before a fix is considered done.
