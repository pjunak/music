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

From `backend/`, check the bundle with:

```powershell
python -m app.reference_contracts --check
```

Regeneration is deliberate and reviewable:

```powershell
python -m app.reference_contracts --write
```

Later Phase 1 fixtures add observed HTTP/WebSocket behavior, synthetic database records, legacy
device imports, mode YAML round trips, media ranges, and normalized differential scenarios. Schema
snapshots alone are not parity proof.
