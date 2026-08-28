# Rust runtime third-party notices

This file records non-permissive dependencies that need explicit handling when the Rust candidate
is distributed as an executable or container. It is an operational compliance record, not legal
advice. The complete dependency graph remains enforced by `cargo deny check`.

## dyn-eq 0.1.3

`dyn-eq` is an unmodified transitive dependency of tract and is licensed under the Mozilla Public
License 2.0. Its source is available from the
[`dyn-eq` 0.1.3 crate](https://crates.io/crates/dyn-eq/0.1.3) and the
[`dyn-eq` repository](https://github.com/Rayzeq/dyn-eq). The license text and distribution terms are
available from the [Mozilla Public License 2.0](https://www.mozilla.org/MPL/2.0/).

Project-owned source files do not incorporate `dyn-eq` source. If the final release packages a Rust
binary, its accompanying notices must point recipients to the exact `dyn-eq` source and must not
restrict their MPL-2.0 rights. If the dependency is modified or its version changes, re-review this
exception before release.

## Operator-supplied voice model

The `voice_instrumental-musicnn-msd-2.pb` weights are not part of the application image or source
tree. Operators obtain them separately under their CC BY-NC-SA 4.0 terms. The application verifies
the exact supported SHA-256 and never downloads or rewrites the model.
