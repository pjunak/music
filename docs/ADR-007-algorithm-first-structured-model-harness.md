# ADR-007: Algorithm-first, schema-bound model harness

**Status:** Accepted

**Date:** 2026-08-22

**Decider:** Project owner

## Context

The first model-backed Assistant tasks already validated JSON locally, but each task
hand-wrote its own output description and example. JSON-object response mode could
request an object without carrying the actual validator schema to the provider, and
local analysis was not consistently represented as evidence before a model call.
This made prompt/schema drift, avoidable provider work, and ambiguous evidence
priority more likely.

The product must remain useful without a provider, preserve provider choice, minimize
disclosure and cost, and keep every generated result review-only.

## Decision

Use one versioned structured harness for every implemented model task:

```text
indexed metadata / local audio / operator request
                    |
                    v
        deterministic evidence and safety rules
                    |
                    v
        local candidate, baseline, or cleanup result
                    |
                    v
        bounded task-specific provider document
                    |
                    v
       exact JSON Schema response + strict validation
                    |
                    v
       local identity, bounds, and source reconstruction
                    |
                    v
            explicit operator review/commit
```

### Shared request contract

- A `StructuredTaskDefinition` owns the task role, objective, untrusted-data
  boundary, and ordered decision rules.
- The output JSON Schema is generated from the same strict Pydantic model used for
  local validation. A locally validated example and that schema are embedded in the
  fixed system prompt and carried on `StructuredModelRequest`. A task may close that
  generated schema further with request-specific identity enums and exact list bounds;
  the transformed schema is still shared by the prompt and provider request.
- The standard `openai-compatible/v1` adapter uses broadly supported JSON-object
  response mode. The `openai-compatible-json-schema/v1` adapter sends the exact
  schema with `response_format.type=json_schema` and `strict=true` for providers
  that document support. It fails before network I/O when a task omitted its schema.
- Models return one JSON object only. Markdown, prose, arrays, type coercion,
  additional fields, incomplete output, unknown identities, and values outside local
  constraints fail closed. The server does not repair or reinterpret authoritative
  IDs, mappings, ordering, confidence, or numeric decisions. Task contracts may
  deterministically truncate bounded incidental review prose such as tagging evidence,
  cleanup reasons, or EQ rationale/cautions; this never changes the core decision.
- The harness and per-role feature-contract versions participate in the runtime
  fingerprint. A change makes conformance and quality certification stale.

### Task-specific local work

- **Playlist planning:** local code owns eligibility, exclusions, bounded candidate
  reduction, evidence separation, scores, default duration selection, and energy
  sequencing. The model receives that baseline and can return only ranked and
  selected IDs from an exact request-specific enum. The example starts from the local
  plan rather than an empty response. Public data and reasons are reconstructed locally.
- **Music tagging:** the server derives a `local-metadata-evidence/v3`
  hypothesis with controlled-tag ID, matched-field, matched-term, and canonical-title
  provenance from the same descriptive fields and canonical library-relative path sent
  to the provider. Paths and metadata are labelled untrusted; the absolute media root
  and paths outside the indexed library never cross the provider boundary. Current
  `local-audio/v1` evidence is reduced to bounded axes, tempo, activity, dynamic range, rhythmic
  density, rhythmic stability, and confidence. These are labelled hypotheses and
  signal proxies; they cannot establish semantic setting or scene context by themselves.
  A revisioned operator vocabulary supplies stable IDs, names, groups, definitions,
  exact cleanup aliases, and overlapping context cues. The complete compact ID/name/group
  index is sent once per batch; detailed definitions and aliases are sent only for the
  locally highlighted candidates. Context cues stay local and may highlight related tags
  across groups without becoming cleanup mappings. Each candidate identifies which
  matched terms came only from context cues and whether one or several independent
  metadata fields support the candidate. Corroborated candidates are ordered first so
  isolated title or artist words do not anchor fast models ahead of the surrounding
  context. Candidate and term lists are bounded deterministically, with exact matches
  retained before cue-only matches when support is otherwise equal. The local hypothesis does not remove
  other operator-approved choices. The model returns only exact IDs; names are restored
  locally.
- **Manual-tag cleanup:** declared aliases plus deterministic spelling and plural rules
  run first. Those suggestions require no provider request. The model sees only
  unresolved sources plus canonical ID definitions, then must return one ordered
  canonical-ID-or-null decision per source. The combined proposal labels each
  suggestion as `local-rule` or `model` and is bound to both catalog and vocabulary.
- **EQ drafting:** a deterministic intent map creates a conservative ten-band
  baseline and a per-band refinement envelope. The model can refine only inside that
  envelope in 0.5 dB steps; the server owns frequencies, safety checks, and preset
  construction.

## Reliability and scale

- Local preprocessing is deterministic, bounded by the existing durable jobs, and
  reusable after browser refresh.
- Provider calls remain off the event loop, bounded by request/response size and role
  time/token limits, non-restartable after uncertain cost, and usage-checkpointed.
- There is no automatic provider retry or schema-repair call. Either could duplicate
  cost and hide an incompatible model. The operator deliberately retries after
  reviewing the failure.
- Library scale is contained before the provider boundary: at most 100 playlist
  candidates, 20 tagging tracks per batch, and 500 catalog tags in cleanup batches of
  at most 50 unresolved names. Deterministic cleanup can reduce those payloads or avoid
  provider calls completely.
- The tagging quality suite submits four synthetic tracks per request. This exercises
  the live multi-track contract while evaluating and reporting every scenario
  independently.

## Trade-offs

- Embedding JSON Schema increases input tokens, but removes a second hand-maintained
  description and gives compatible providers a native constraint.
- Sending the compact vocabulary index plus candidate details costs input tokens, but
  removes the inefficient create-tags-then-interpret-tags loop and makes unknown model
  output unrepresentable for strict-schema providers. Keeping non-candidate definitions
  local avoids repeating the full 131-tag reference for every small batch.
- The strict adapter improves format reliability but is not universally supported;
  keeping it explicit avoids speculative retries or provider-name detection.
- Supplying local hypotheses can anchor a model. The prompt therefore labels source
  authority and uncertainty, excludes manual/review state from tagging, and retains
  independent provenance in storage and UI.
- Conservative EQ envelopes may reject a creative but valid curve. Safety and
  predictability are preferred because this feature produces review drafts, not
  mastering decisions.

## Revisit when

- a second provider protocol needs a native structured-output adapter;
- a provider-neutral inference-parameter contract is demonstrated across supported
  services;
- specialized audio input receives its own bounded upload, consent, evaluation, and
  retention contract; or
- measured quality shows that local evidence gates should skip additional model calls.
