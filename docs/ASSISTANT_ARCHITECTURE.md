# Assistant architecture and contract map

**Status:** Living documentation
**Last audited:** 2026-08-25

This is the current map for the local-first Assistant and its optional model workflows. Use it to
find ownership, privacy boundaries, contract versions, evaluation gates, and regression tests.
The practical deployment and acceptance sequence remains in [the operator guide](../ASSISTANT.md);
the reasons behind durable choices remain in the linked ADRs.

## Source-of-truth order

When two descriptions disagree, resolve the disagreement in the same change:

1. strict Pydantic models, provider definitions, and registered job/API code define current runtime
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
- Runtime fingerprints include the shared harness and role contract. Relevant changes make saved
  conformance and quality results stale instead of silently reusing them.
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
 strict parse -> Pydantic -> identity/bounds validation
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
| Task prompt, example, schema, untrusted-data labels | [`structured_harness.py`](../backend/app/assistant/structured_harness.py) | [`test_assistant_structured_harness.py`](../backend/tests/test_assistant_structured_harness.py) | [ADR-007](ADR-007-algorithm-first-structured-model-harness.md) |
| Adapter/capability/role inventory and runtime fingerprints | [`providers/definitions.py`](../backend/app/assistant/providers/definitions.py) | [`test_assistant_providers.py`](../backend/tests/test_assistant_providers.py) | [ADR-001](ADR-001-assistant-provider-connections.md), [ADR-002](ADR-002-assistant-model-execution.md) |
| Request execution and response parsing | [`providers/execution.py`](../backend/app/assistant/providers/execution.py) | [`test_assistant_provider_execution.py`](../backend/tests/test_assistant_provider_execution.py) | [ADR-002](ADR-002-assistant-model-execution.md), [ADR-007](ADR-007-algorithm-first-structured-model-harness.md) |
| URL validation, SSRF boundary, redirect refusal, byte/time limits | [`providers/transport.py`](../backend/app/assistant/providers/transport.py) | [`test_assistant_provider_verification.py`](../backend/tests/test_assistant_provider_verification.py) | [ADR-001](ADR-001-assistant-provider-connections.md) |
| Credential encryption, initialization, reset, and offline rotation | [`providers/credentials.py`](../backend/app/assistant/providers/credentials.py), [`providers/credential_admin.py`](../backend/app/assistant/providers/credential_admin.py) | [`test_assistant_credential_admin.py`](../backend/tests/test_assistant_credential_admin.py), [`test_assistant_providers.py`](../backend/tests/test_assistant_providers.py) | [ADR-001](ADR-001-assistant-provider-connections.md) |
| Role preparation and stale-gate enforcement | [`providers/service.py`](../backend/app/assistant/providers/service.py), [`model_evaluation.py`](../backend/app/assistant/model_evaluation.py) | [`test_assistant_providers.py`](../backend/tests/test_assistant_providers.py) | [ADR-004](ADR-004-durable-model-quality-gates.md) |
| Attempt/token accounting | [`providers/usage.py`](../backend/app/assistant/providers/usage.py) | [`test_assistant_provider_usage.py`](../backend/tests/test_assistant_provider_usage.py) | [ADR-004](ADR-004-durable-model-quality-gates.md) |
| Durable job lifecycle | [`jobs/`](../backend/app/jobs), task-specific job modules | [`test_jobs.py`](../backend/tests/test_jobs.py) | [Repository persistence rules](../AGENTS.md#persistence-and-deployment) |
| Browser API/types and review workflows | [`frontend/src/core/api.ts`](../frontend/src/core/api.ts), [`frontend/src/views/assistant/`](../frontend/src/views/assistant) | colocated Vitest files | [Assistant UX philosophy](assistant-ux-philosophy.md) |

## Current contract inventory

The exact values below are intentionally checked by
[`test_documentation.py`](../backend/tests/test_documentation.py). Update the owning code, this
table, its evaluation suite, disclosure copy, and tests together.

Shared contracts:

- harness: `assistant-structured-harness/v2`
- provider conformance: `assistant-provider-conformance/v3`
- standard adapter: `openai-compatible/v1`
- strict-schema adapter: `openai-compatible-json-schema/v1`

| Role | Runtime fingerprint fragment | Disclosure | Engine/storage identity | Quality gate | Live job |
|---|---|---|---|---|---|
| Playlist planning (`playlist_planner`) | `assistant-playlist-planner-input/v2+output/v1+closed-ids/v1` | `assistant-playlist-model-disclosure/v2` | `model-playlist-planner/v2` | `playlist-quality-v1` | `assistant.model-playlist-suggestion` |
| Mood tagging (`music_tagger`) | `assistant-music-tagger-input/v14+output/v3+local-context/v1` | `assistant-model-music-tagging-disclosure/v10` | `model-context-tagger/v6` | `music-tagging-quality-v1` | `assistant.model-music-tagging` |
| Mood-tag cleanup (`tag_cleanup`) | `assistant-model-tag-cleanup-input/v3+output/v2+incidental-text-bounds/v1` | `assistant-model-tag-cleanup-disclosure/v3` | `model-tag-cleanup/v3` | `tag-cleanup-quality-v1` | `assistant.model-tag-cleanup` |
| EQ assistance (`eq_assistant`) | `assistant-eq-draft-input/v2+output/v1+incidental-text-bounds/v1` | `assistant-eq-draft-disclosure/v2` | `model-graphic-eq/v2` | `eq-quality-v1` | `assistant.model-eq-draft` |

Full task output contracts are `assistant-playlist-planner-output/v1`,
`assistant-music-tagger-output/v3`, `assistant-model-tag-cleanup-output/v2`, and
`assistant-eq-draft-output/v1`. Reserved roles `library_cleanup` and `audio_analyzer` use
`reserved-library-cleanup/v1` and `reserved-audio-analyzer/v1`; they are visible but not
configurable.

## Workflow traceability

| Workflow | Local authority and provider contract | Durable/API layer | Suite and regression tests | Decision record |
|---|---|---|---|---|
| Playlist draft | [`local.py`](../backend/app/assistant/local.py), [`model_playlist.py`](../backend/app/assistant/model_playlist.py) | [`model_suggestions.py`](../backend/app/assistant/model_suggestions.py), [`api/assistant.py`](../backend/app/api/assistant.py) | [`playlist-local-v1.json`](../backend/app/assistant/evaluation_suites/playlist-local-v1.json), [`test_assistant_model_playlist.py`](../backend/tests/test_assistant_model_playlist.py), [`test_assistant_model_suggestions.py`](../backend/tests/test_assistant_model_suggestions.py) | [ADR-003](ADR-003-hybrid-model-playlist-evaluation.md), [ADR-005](ADR-005-consent-bound-model-playlist-suggestions.md), [ADR-007](ADR-007-algorithm-first-structured-model-harness.md) |
| Local track context | [`audio_context.py`](../backend/app/assistant/audio_context.py), [`library_context.py`](../backend/app/assistant/library_context.py), optional [`voice_analysis.py`](../backend/app/assistant/voice_analysis.py) | [`api/assistant.py`](../backend/app/api/assistant.py) | [`test_assistant_library_context.py`](../backend/tests/test_assistant_library_context.py), [`test_assistant_voice_analysis.py`](../backend/tests/test_assistant_voice_analysis.py) | [ADR-008](ADR-008-comprehensive-local-track-context.md), [ADR-009](ADR-009-opt-in-local-voice-analysis.md) |
| Mood-tag suggestion and review | [`model_tagger.py`](../backend/app/assistant/model_tagger.py), [`tag_vocabulary.py`](../backend/app/assistant/tag_vocabulary.py) | [`model_tagging.py`](../backend/app/assistant/model_tagging.py), [`api/assistant_tags.py`](../backend/app/api/assistant_tags.py), [`tag_reviews.py`](../backend/app/assistant/tag_reviews.py) | [`music-tagging-v1.json`](../backend/app/assistant/evaluation_suites/music-tagging-v1.json), [`test_assistant_model_tagging.py`](../backend/tests/test_assistant_model_tagging.py), [evaluation guide](../backend/evaluation/MUSIC_TAGGING.md) | [ADR-006](ADR-006-review-only-model-music-tagging.md), [ADR-007](ADR-007-algorithm-first-structured-model-harness.md), [ADR-008](ADR-008-comprehensive-local-track-context.md) |
| Mood-tag cleanup | [`tag_cleanup.py`](../backend/app/assistant/tag_cleanup.py), [`model_tag_cleanup.py`](../backend/app/assistant/model_tag_cleanup.py) | [`model_tag_cleanup_job.py`](../backend/app/assistant/model_tag_cleanup_job.py), [`api/assistant_tags.py`](../backend/app/api/assistant_tags.py) | [`tag-cleanup-v1.json`](../backend/app/assistant/evaluation_suites/tag-cleanup-v1.json), [`test_assistant_model_tag_cleanup.py`](../backend/tests/test_assistant_model_tag_cleanup.py) | [ADR-007](ADR-007-algorithm-first-structured-model-harness.md) |
| EQ draft | [`model_eq.py`](../backend/app/assistant/model_eq.py) | [`model_eq_job.py`](../backend/app/assistant/model_eq_job.py), [`api/assistant.py`](../backend/app/api/assistant.py) | [`eq-assistant-v1.json`](../backend/app/assistant/evaluation_suites/eq-assistant-v1.json), [`test_assistant_model_eq.py`](../backend/tests/test_assistant_model_eq.py) | [ADR-007](ADR-007-algorithm-first-structured-model-harness.md) |
| Shared quality certification | [`model_evaluation.py`](../backend/app/assistant/model_evaluation.py) | [`api/assistant_providers.py`](../backend/app/api/assistant_providers.py) | [`test_assistant_model_evaluation_assets.py`](../backend/tests/test_assistant_model_evaluation_assets.py), task tests above | [ADR-004](ADR-004-durable-model-quality-gates.md) |

## Provider disclosure boundaries

| Workflow | Sent | Kept local |
|---|---|---|
| Playlist | operator request and at most 100 locally eligible, path-free candidates with bounded evidence and the local plan | library paths, excluded tracks, credentials, playlists, review history, final public reconstruction |
| Mood tagging | at most 20 tracks per request; bounded indexed metadata, canonical library-relative path as untrusted data, complete controlled vocabulary, optional bounded current context | absolute media root, paths outside the index, audio, waveforms, spectrograms, full timelines, database mood tags, generated suggestions, reviews, credentials |
| Mood-tag cleanup | unresolved source IDs/names and usage counts, canonical vocabulary IDs/names/groups/definitions | track metadata, paths, audio, playlists, generated tags, review history, credentials |
| EQ | operator goal, fixed ten-band frequencies, deterministic baseline guidance, per-band limits | songs, audio, library metadata, paths, playlists, existing presets, credentials, final preset document |

The concrete `shared_with_provider` and `never_shared` lists returned by each availability endpoint
are the consent surface. Any data-category change requires a new disclosure version and invalidates
prior consent.

## Safe change procedure

1. Identify whether the change affects only local evidence, provider input, provider output,
   disclosure, storage identity, execution transport, or review/commit behavior.
2. Change the strict model and local validators first. Generate the provider schema from that same
   model; do not hand-maintain a second schema description.
3. Advance the smallest owning contract version. If runtime behavior or harness semantics changed,
   also update `MODEL_ROLE_RUNTIME_CONTRACTS` or the shared harness version so old gates become stale.
4. Update the fixed task example, request-specific schema closure, synthetic suite, negative cases,
   and privacy assertions. Never use private library data as a checked-in fixture.
5. Update the task disclosure when any sent/retained data category or retry/cost boundary changes.
6. Update this inventory and amend the relevant ADR when the reasoning or trade-off changed.
7. Run the narrow task tests, documentation tests, full backend gates, frontend gates, and a real
   provider conformance/quality run before enabling the changed configuration in production.
8. Treat real-provider and real-audio checks as manual acceptance. Passing mocked automation does
   not establish compatibility with a provider or accuracy on the operator's library.

## External foundations

The harness-specific standards, vendor behavior, security guidance, and the reasoning derived from
them are recorded beside the decision in [ADR-007](ADR-007-algorithm-first-structured-model-harness.md#sources-and-rationale).
Those references support the architecture; they do not replace local validation, synthetic quality
suites, provider conformance, or explicit human review.
