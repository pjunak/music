# Assistant architecture and contract map

**Status:** Living documentation
**Last audited:** 2026-08-30

This is the current map for the local-first Assistant and its optional model workflows. Use it to
find ownership, privacy boundaries, contract versions, evaluation gates, and regression tests.
The practical deployment and acceptance sequence remains in [the operator guide](../ASSISTANT.md);
the reasons behind durable choices remain in the linked ADRs.

## Source-of-truth order

When two descriptions disagree, resolve the disagreement in the same change:

1. strict Rust task types, provider definitions, and registered job/API code define current runtime
   behavior;
2. tests and checked-in synthetic suites define executable acceptance boundaries;
3. this map records current ownership and version inventory;
4. ADRs record why a decision was made and may contain historical version strings;
5. the operator guide describes deployment and human acceptance.

The documentation tests intentionally bind this file to the runtime contract constants. They do
not prove model quality or provider compatibility; the task-specific quality and conformance gates
remain authoritative for those claims.

## Non-negotiable boundaries

- Local analysis, filtering, identity, and safety limits remain authoritative. A provider may refine
  a bounded draft but cannot invent an executable action or a second state machine.
- Provider input is task-specific, minimized, size-bounded, and disclosed before live-library use.
  Metadata and paths inside the JSON user document are untrusted data, never instructions.
- Provider output is untrusted. It must parse as one object, pass the strict task model, satisfy
  request-specific identity/bounds checks, and be reconstructed from local source data where
  applicable.
- Model results remain inert until explicit operator review. They never write playlists, presets,
  files, embedded metadata, or database mood tags directly.
- Connection verification, role conformance, quality certification, and live-run consent are
  separate gates. Passing one does not imply another.
- Runtime fingerprints include the shared harness and role contract plus a SHA-256 digest of each
  role's executable prompt/schema modules and checked-in evaluation suites. Relevant source or
  suite changes make saved conformance and quality results stale even when a developer forgets to
  advance the human-readable contract fragment.
- Provider jobs are non-restartable after uncertain external cost. Usage is checkpointed after
  every attempt, including attempts that later fail.

## End-to-end flow

```text
operator request / indexed library / local audio
                    |
                    v
       local analysis, filtering, and baseline
                    |
                    v
       disclosed, bounded task input document
                    |
                    v
   structured harness: prompt + example + JSON Schema
                    |
                    v
  explicit provider adapter + verified role configuration
                    |
                    v
 strict Serde parse -> identity/bounds validation
                    |
                    v
  local reconstruction -> durable review-only result
                    |
                    v
        explicit preview / selection / commit
```

## Shared platform map

| Concern | Current owner | Executable evidence | Rationale |
|---|---|---|---|
| Task prompt, example, schema, untrusted-data labels | [`structured_harness.rs`](../crates/music-application/src/assistant/structured_harness.rs) | colocated strict-shape and bounds tests | [ADR-007](ADR-007-algorithm-first-structured-model-harness.md) |
| Adapter/capability/role inventory and runtime fingerprints | [`providers.rs`](../crates/music-application/src/assistant/providers.rs), [`runtime_contract.rs`](../crates/music-application/src/assistant/runtime_contract.rs) | colocated inventory and digest tests | [ADR-001](ADR-001-assistant-provider-connections.md), [ADR-002](ADR-002-assistant-model-execution.md) |
| Provider-specific model IDs, request schemas, inference parameters, and response shapes | [`provider_handlers.rs`](../crates/music-server/src/provider_handlers.rs) | transport-free production-shaped request and parser tests | [ADR-011](ADR-011-in-process-provider-adapter-handlers.md) |
| Bounded request execution | [`provider_transport.rs`](../crates/music-server/src/provider_transport.rs), task types under [`assistant/`](../crates/music-application/src/assistant) | local fixture-server, strict parsing, and bounds tests | [ADR-002](ADR-002-assistant-model-execution.md), [ADR-007](ADR-007-algorithm-first-structured-model-harness.md), [ADR-011](ADR-011-in-process-provider-adapter-handlers.md) |
| URL validation, SSRF boundary, redirect refusal, byte/time limits | [`provider_transport.rs`](../crates/music-server/src/provider_transport.rs) | pinned-DNS, special-range, redirect, timeout, and response-limit tests | [ADR-001](ADR-001-assistant-provider-connections.md) |
| Credential encryption, initialization, reset, and offline rotation | [`crypto.rs`](../crates/music-storage/src/crypto.rs), [`provider_credentials.rs`](../crates/music-server/src/provider_credentials.rs), [`providers.rs`](../crates/music-storage/src/providers.rs) | Python-compatibility fixture plus reset/rotation transaction tests | [ADR-001](ADR-001-assistant-provider-connections.md) |
| Role preparation and stale-gate enforcement | [`providers.rs`](../crates/music-application/src/assistant/providers.rs), [`provider_api.rs`](../crates/music-server/src/provider_api.rs) | role fingerprint, conformance, quality, and active-job tests | [ADR-004](ADR-004-durable-model-quality-gates.md) |
| Attempt/token accounting | [`provider_usage.rs`](../crates/music-application/src/assistant/provider_usage.rs), [`model_jobs.rs`](../crates/music-server/src/model_jobs.rs) | colocated usage and checkpoint tests | [ADR-004](ADR-004-durable-model-quality-gates.md) |
| Durable job lifecycle | [`jobs.rs`](../crates/music-application/src/jobs.rs), [`jobs.rs`](../crates/music-storage/src/jobs.rs), [`jobs.rs`](../crates/music-server/src/jobs.rs) | persisted-boundary fault tests in `music-storage` | [Repository persistence rules](../AGENTS.md#persistence-and-deployment) |
| Browser API/types and review workflows | [`frontend/src/core/api.ts`](../frontend/src/core/api.ts), [`frontend/src/views/assistant/`](../frontend/src/views/assistant) | colocated Vitest files | [Assistant UX philosophy](assistant-ux-philosophy.md) |

## Current contract inventory

The exact values below are Rust constants included in the executable runtime fingerprint. Update
the owning code, this table, its evaluation suite, disclosure copy, and tests together.

Shared contracts:

- harness: `assistant-structured-harness/v3`
- provider conformance result: `assistant-provider-conformance/v3`
- provider conformance challenge: `assistant-provider-conformance-challenge/v4`
- OpenAI Responses adapter: `openai-responses/v1`
- standard adapter: `openai-compatible/v1`
- strict-schema adapter: `openai-compatible-json-schema/v1`
- Google Gemini adapter: `google-gemini-openai/v1`
- Google Gemini strict-schema adapter: `google-gemini-openai-json-schema/v1`

Use `openai-responses/v1` with the exact base URL `https://api.openai.com/v1` for OpenAI. It sends
native Responses requests with `max_output_tokens`, `reasoning.effort`, and the task schema under
`text.format`; its wire projection removes only unsupported `uniqueItems` while keeping OpenAI's
supported array and string constraints. The generic adapters are reserved for third-party
OpenAI-compatible services.
Both Gemini adapter IDs use the exact base URL
`https://generativelanguage.googleapis.com/v1beta/openai`, canonicalize `models/` resource IDs,
send Google's integration-identification header, and constrain results with a Gemini-compatible
projection of the task's JSON Schema. The complete generated schema remains in the fixed prompt and
is always enforced by the task's local strict Rust validation; the provider projection removes only
JSON Schema keywords outside Gemini's documented structured-output subset.
The older Gemini strict-schema ID remains a saved-connection compatibility alias. Provider error
payloads may contribute only allowlisted machine codes; upstream messages never reach diagnostics.

| Role | Runtime fingerprint fragment | Disclosure | Engine/storage identity | Quality gate | Live job |
|---|---|---|---|---|---|
| Playlist planning (`playlist_planner`) | `assistant-playlist-planner-input/v2+output/v1+closed-ids/v1` | `assistant-playlist-model-disclosure/v2` | `model-playlist-planner/v2` | `playlist-quality-v1` | `assistant.model-playlist-suggestion` |
| Mood tagging (`music_tagger`) | `assistant-music-tagger-input/v19+output/v3+local-context/v2` | `assistant-model-music-tagging-disclosure/v11` | `model-context-tagger/v6` | `music-tagging-quality-v1` | `assistant.model-music-tagging` |
| Mood-tag cleanup (`tag_cleanup`) | `assistant-model-tag-cleanup-input/v3+output/v2+incidental-text-bounds/v1` | `assistant-model-tag-cleanup-disclosure/v3` | `model-tag-cleanup/v3` | `tag-cleanup-quality-v1` | `assistant.model-tag-cleanup` |
| EQ assistance (`eq_assistant`) | `assistant-eq-draft-input/v2+output/v1+incidental-text-bounds/v1` | `assistant-eq-draft-disclosure/v2` | `model-graphic-eq/v2` | `eq-quality-v1` | `assistant.model-eq-draft` |

Full task output contracts are `assistant-playlist-planner-output/v1`,
`assistant-music-tagger-output/v3`, `assistant-model-tag-cleanup-output/v2`, and
`assistant-eq-draft-output/v1`. Reserved roles `library_cleanup` and `audio_analyzer` use
`reserved-library-cleanup/v1` and `reserved-audio-analyzer/v1`; they are visible but not
configurable. `library_cleanup` is presented only in the task-specific **Library cleanup →
AI assistance** tab; reusable provider connections remain under Assistant setup.

The Library cleanup workspace preserves a separate local authority boundary. The local engine
produces filename, folder, and embedded-tag proposals; `cleanup_batches` journals only explicitly
selected writes. **History & rollback** reads those server journals, downloads the complete JSON,
and invokes the existing conflict-aware revert path. **Sources** exposes only implemented adapters.
The current `musicbrainz` policy is stored in `cleanup_source_policies`; when disabled, analysis does
not surface online lookups and the verification endpoint performs no network request. Arbitrary URL
scraping is not a supported source contract.

## Workflow traceability

| Workflow | Local authority and provider contract | Durable/API layer | Suite and regression tests | Decision record |
|---|---|---|---|---|
| Playlist draft | [`planner.rs`](../crates/music-application/src/assistant/planner.rs), [`model_playlist.rs`](../crates/music-application/src/assistant/model_playlist.rs) | [`assistant/mod.rs`](../crates/music-server/src/assistant/mod.rs), [`model_jobs.rs`](../crates/music-server/src/model_jobs.rs) | local [`playlist-local-v1.json`](../crates/music-application/src/assistant/evaluation_suites/playlist-local-v1.json), model [`playlist-model-v1.json`](../crates/music-application/src/assistant/evaluation_suites/playlist-model-v1.json), colocated task/runtime tests | [ADR-003](ADR-003-hybrid-model-playlist-evaluation.md), [ADR-005](ADR-005-consent-bound-model-playlist-suggestions.md), [ADR-007](ADR-007-algorithm-first-structured-model-harness.md) |
| Local track context | [`context.rs`](../crates/music-analysis/src/context.rs), [`voice.rs`](../crates/music-analysis/src/voice.rs), [`local_analysis.rs`](../crates/music-application/src/assistant/local_analysis.rs) | [`analysis.rs`](../crates/music-server/src/analysis.rs), [`assistant/mod.rs`](../crates/music-server/src/assistant/mod.rs) | controlled numeric, exact-model, checkpoint, and runtime route tests | [ADR-008](ADR-008-comprehensive-local-track-context.md), [ADR-009](ADR-009-opt-in-local-voice-analysis.md), [ADR-014](ADR-014-perceptual-context-measurements.md) |
| Mood-tag suggestion and review | [`model_tagger.rs`](../crates/music-application/src/assistant/model_tagger.rs), [`vocabulary.rs`](../crates/music-application/src/assistant/vocabulary.rs), [`tags.rs`](../crates/music-application/src/assistant/tags.rs) | [`model_jobs.rs`](../crates/music-server/src/model_jobs.rs), [`assistant/mod.rs`](../crates/music-server/src/assistant/mod.rs), storage review transactions | [`music-tagging-v1.json`](../crates/music-application/src/assistant/evaluation_suites/music-tagging-v1.json) and colocated contract/runtime tests | [ADR-006](ADR-006-review-only-model-music-tagging.md), [ADR-007](ADR-007-algorithm-first-structured-model-harness.md), [ADR-008](ADR-008-comprehensive-local-track-context.md) |
| Mood-tag cleanup | [`model_tag_cleanup.rs`](../crates/music-application/src/assistant/model_tag_cleanup.rs), [`tags.rs`](../crates/music-application/src/assistant/tags.rs) | [`model_jobs.rs`](../crates/music-server/src/model_jobs.rs), [`assistant/mod.rs`](../crates/music-server/src/assistant/mod.rs) | [`tag-cleanup-v1.json`](../crates/music-application/src/assistant/evaluation_suites/tag-cleanup-v1.json) and colocated strict-result tests | [ADR-007](ADR-007-algorithm-first-structured-model-harness.md) |
| EQ draft | [`model_eq.rs`](../crates/music-application/src/assistant/model_eq.rs) | [`model_jobs.rs`](../crates/music-server/src/model_jobs.rs), [`assistant/mod.rs`](../crates/music-server/src/assistant/mod.rs) | [`eq-assistant-v1.json`](../crates/music-application/src/assistant/evaluation_suites/eq-assistant-v1.json) and envelope/runtime tests | [ADR-007](ADR-007-algorithm-first-structured-model-harness.md) |
| Shared quality certification | [`model_quality.rs`](../crates/music-application/src/assistant/model_quality.rs) | [`provider_api.rs`](../crates/music-server/src/provider_api.rs), [`model_jobs.rs`](../crates/music-server/src/model_jobs.rs) | fixed suites plus complete/retest identity tests | [ADR-004](ADR-004-durable-model-quality-gates.md) |

## Provider disclosure boundaries

| Workflow | Sent | Kept local |
|---|---|---|
| Playlist | operator request and at most 100 locally eligible, path-free candidates with bounded evidence and the local plan | library paths, excluded tracks, credentials, playlists, review history, final public reconstruction |
| Mood tagging | at most 20 tracks per request; bounded artist, album, origin, and genre metadata, complete controlled vocabulary, optional bounded current context | track and display titles, file and folder names, every library path, audio, waveforms, spectrograms, full timelines, database mood tags, generated suggestions, reviews, credentials |
| Mood-tag cleanup | unresolved source IDs/names and usage counts, canonical vocabulary IDs/names/groups/definitions | track metadata, paths, audio, playlists, generated tags, review history, credentials |
| EQ | operator goal, fixed ten-band frequencies, deterministic baseline guidance, per-band limits | songs, audio, library metadata, paths, playlists, existing presets, credentials, final preset document |

Playlist retrieval keeps the original local rank, then appends additional eligible candidates
found through controlled-vocabulary aliases and semantic cues, up to the same 100-candidate
disclosure limit. Canonical display titles override conflicting raw scanner titles; artist names
and filesystem paths remain searchable evidence but cannot create mood axes. Candidate percentages
shown after model ranking are explicitly labeled as local evidence, not model confidence.

Quality suites exercise the same production request shape: 20-track tagging batches,
20-source cleanup batches, and the 100-candidate playlist boundary. Playlist certification
also scores target-duration error and selected-artist diversity, and requires one semantic
uplift case that cannot pass by merely echoing the local tie order. EQ includes semantic goals
outside its deterministic keyword rules so copying the baseline is not sufficient.

The concrete `shared_with_provider` and `never_shared` lists returned by each availability endpoint
are the consent surface. Any data-category change requires a new disclosure version and invalidates
prior consent.

## Safe change procedure

1. Identify whether the change affects only local evidence, provider input, provider output,
   disclosure, storage identity, execution transport, or review/commit behavior.
2. Change the strict Rust type and local validators first. Generate the provider schema from that same
   model; do not hand-maintain a second schema description.
3. Advance the smallest owning contract version. If runtime behavior or harness semantics changed,
   also update `MODEL_ROLE_RUNTIME_CONTRACTS` or the shared harness version so old gates become stale.
4. Update the fixed task example, request-specific schema closure, synthetic suite, negative cases,
   and privacy assertions. Never use private library data as a checked-in fixture.
5. Update the task disclosure when any sent/retained data category or retry/cost boundary changes.
6. Put provider-specific model-ID, endpoint, schema-dialect, inference-parameter, or response-shape
   differences in a versioned handler. Keep network I/O in the shared transport, update the handler
   fingerprint coverage, and never select behavior from a connection name, URL, or model-name guess.
7. Update this inventory and amend the relevant ADR when the reasoning or trade-off changed.
8. Run the narrow task tests, documentation checks, full Rust workspace gates, frontend gates, and a real
   provider conformance/quality run before enabling the changed configuration in production.
9. Treat real-provider and real-audio checks as manual acceptance. Passing mocked automation does
   not establish compatibility with a provider or accuracy on the operator's library.

## External foundations

The harness-specific standards, vendor behavior, security guidance, and the reasoning derived from
them are recorded beside the decision in [ADR-007](ADR-007-algorithm-first-structured-model-harness.md#sources-and-rationale).
Those references support the architecture; they do not replace local validation, synthetic quality
suites, provider conformance, or explicit human review.
