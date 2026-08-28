# Rewrite reference contracts

These checked-in artifacts freeze the declared Python contracts at the Rust rewrite baseline. They
are generated from model metadata only and contain no runtime database rows, credentials, device
registry, absolute storage paths, or media.

`v1/manifest.json` records artifact hashes and the expected counts. The bundle covers:

- every declared HTTP operation and OpenAPI component schema;
- every client-to-server WebSocket action and server-to-client message;
- canonical valid/defaulted examples and representative rejection cases for those WebSocket DTOs;
- the complete SQLAlchemy SQLite DDL, including foreign keys, unique constraints, and indexes; and
- the current mode, soundboard, cue, and preset schemas.

From the repository root, verify the immutable baseline, generated Rust contracts, and semantic
OpenAPI compatibility report with:

```powershell
cargo run --locked -p music-server --bin music-cli -- contracts check --root .
```

The Python baseline is no longer regenerated on the rewrite branch. An owner-approved contract
break must create a new reference version from the preserved legacy revision, never silently edit
`v1`. Rust-owned generated artifacts are refreshed with:

```powershell
cargo run --locked -p music-server --bin music-cli -- contracts export --root .
```

Rust tests consume this corpus directly and add observed HTTP/WebSocket behavior, representative
database migration, legacy-device import, mode YAML round trips, media ranges, metadata mutation,
authentication/crypto, and normalized compatibility scenarios. Schema snapshots alone are not
parity proof; the runtime integration tests and generated semantic report are part of the gate.
