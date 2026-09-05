# ADR-018: Derive model schemas and isolate catalog transport

**Status:** Accepted
**Date:** 2026-09-05
**Scope:** Approved quality-audit implementation, second batch

## Context

Four Rust model tasks independently maintained Serde output types and JSON Schema
field definitions. The copies could disagree: cleanup's schema required an explicit
target ID or null, while deserializing an Option accepted a missing target field.
The schema also omitted the half-decibel step already required by EQ validation.

Catalog enrichment mixed source policy, identity scoring, cache validity, vocabulary
mapping, review proposal persistence, HTTP parsing, and local fingerprint execution
in the server crate. Malformed Last.fm and AcoustID collection responses could be
mistaken for valid empty evidence and cached.

## Decision

Use Schemars on the same strict result structs and confidence enums that Serde
deserializes. Generate Draft 2020-12 schemas with inline nested types. Remove Rust
type titles and numeric format annotations before the existing adapter projection.
Add only task constraints and request-specific allowed IDs to this generated shape.
Represent cleanup targets with a required untagged string-or-null enum, so omitted
targets fail in both the schema and the production result handler.

Local validation remains authoritative for constraints spanning fields: exact unique
track membership, ordered cleanup decisions, selected playlist IDs being ranked,
EQ per-band envelopes, and reconstruction from local metadata. Incidental review
prose may still be truncated by the documented task policy; numeric values and IDs
are never repaired. The schema now also declares EQ's existing 0.5 dB step.

Use a test-only JSON Schema validator with network and file resolution features
disabled. Compare it with each actual result handler after removing every required
field, adding unknown fields, and changing nested types. Test dynamic invented and
duplicate IDs separately, and explicitly test the relational and prose exceptions.

Move catalog workflow and deterministic mapping into `music-application` behind the
typed `CatalogConnector` port. The connector returns candidate recordings, release
details, acoustic candidates, and community tags. The application computes local
scores, selects identity, controls fallback, maps exact vocabulary terms, holds
source leases, checks evidence revisions, caches complete results, and writes
review-only proposals. The server retains credential fallback, bounded HTTP parsing,
MusicBrainz pacing, rooted paths, and fingerprint process execution.

Missing response collections are connector errors; explicit empty arrays remain
valid empty results. `catalog-evidence-policy/v2` participates in cache and generated
tag source signatures, expiring old evidence without changing the HTTP result schema
or deleting accepted/manual tags. No database migration is required.

## Alternatives and trade-offs

| Option | Assessment |
|---|---|
| Keep hand-written schemas plus tests | Leaves field names, requiredness, types and enums duplicated; tests alone cannot prevent drift. |
| Derive schemas from Serde result types | Selected. Small runtime schema dependency; independent validator is test-only. Dynamic and relational rules remain explicit. |
| Interpret all task constraints through JSON Schema at runtime | Adds a larger runtime validator and still cannot replace local reconstruction or cross-field policy. |
| Keep catalog orchestration in server | Simpler file layout, but policy is coupled to network/process composition and harder to test independently. |
| Typed application port in the existing process | Selected. Tests can use deterministic observations while exercising real job and storage behavior. No new service or agent framework. |

## Validation and consequences

SQLite-backed job tests cover cache reuse, forced requests, source edits being blocked
during execution, disabled sources preventing requests, ambiguous fingerprint
abstention, metadata-first fallback, partial results being retried only on subsequent
runs, and generated proposals never becoming manual tags automatically. Existing
revision, stale-review, credential, provider-adapter, and compatibility gates remain.

Model runtime fingerprints change, so saved conformance/quality passes expire under
the existing policy. The shared digest also includes the dependency lockfile, so a
schema-generator or parser dependency update cannot silently reuse certification.
Dependency changes conservatively expire all roles; role-only source edits retain
the existing narrow invalidation. Operator-selected models, Thinking settings, disclosure rules,
correction budgets, and quality thresholds are preserved. Release and live provider
acceptance are listed in [AI_ACCEPTANCE.md](AI_ACCEPTANCE.md).

## References

Schemars documents deriving schemas that follow Serde attributes, including strict
objects and renamed fields, in its [official crate documentation](https://docs.rs/schemars/1.2.2/schemars/).
The independent validator's [official documentation](https://docs.rs/jsonschema/0.53.0/jsonschema/)
describes reusable validation and disabling external reference resolution.
