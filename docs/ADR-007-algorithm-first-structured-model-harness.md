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
- **Music tagging:** comprehensive `local-context/v1` analysis remains factual and
  semantic-free. It supplies bounded whole-track trajectories, tempo development, major acoustic
  sections/transitions, repetition, confidence, and optional local voice/instrumental classifier
  evidence or an explicit unknown/unavailable status when current.
  Metadata and canonical library-relative paths are labelled untrusted; the absolute media root,
  audio, waveforms, spectrograms, full timelines, database mood tags, and review state never cross
  the provider boundary. The model receives the full revisioned controlled vocabulary with stable
  IDs, names, groups, definitions, exact aliases, and bounded semantic context cues. The cues are
  global vocabulary guidance that must be confirmed against complete metadata phrases; no local
  per-track candidate-tag hypothesis is sent and the model does not return signal axes. It may
  return only exact IDs, confidence, and bounded
  evidence; names are restored locally. Missing context falls back to conservative metadata/path
  interpretation only when the operator chooses “run anyway”; “skip incomplete” prevents those
  tracks from reaching the provider.
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
- Provider retries are not generic. Mood tagging has one disclosed exception: a
  run-scoped budget permits at most two fresh correction requests after malformed
  JSON, schema-invalid output, a mismatched track set, or an unsupported tag ID.
  The rejected output is never repaired or coerced locally, timeout/network failures
  are not retried, and actual attempts remain usage-checkpointed.
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
- Sending the complete vocabulary index, definitions, aliases, and bounded semantic cues costs
  input tokens, but removes an implicit join the provider could not reliably infer and makes
  expected soundtrack meanings explicit. Exact response enums still make unknown model output
  unrepresentable for strict-schema providers.
- The strict adapter improves format reliability but is not universally supported;
  keeping it explicit avoids speculative retries or provider-name detection.
- Supplying local per-track hypotheses can anchor a model, so none cross the provider boundary.
  Global vocabulary cues are labelled non-exhaustive examples rather than automatic matches;
  the prompt requires confirmation against the full untrusted phrase and retains independent
  provenance in storage and UI.
- Conservative EQ envelopes may reject a creative but valid curve. Safety and
  predictability are preferred because this feature produces review drafts, not
  mastering decisions.

## Sources and rationale

These sources explain the standards and security practices behind the harness. They
support the design; they do not prove that a provider follows a schema or that a model is
safe. Local validation, request-specific identity checks, conformance, synthetic quality
gates, minimized disclosure, and operator review remain required.

| Source | What it establishes | How this project applies it |
|---|---|---|
| [JSON Schema Draft 2020-12](https://json-schema.org/draft/2020-12) | A published vocabulary for describing and validating JSON structure. | Task output has one machine-readable contract. Request-specific enums and list bounds close that contract around the identities in the current request. |
| [Pydantic JSON Schema](https://pydantic.dev/docs/validation/latest/concepts/json_schema/) | `model_json_schema()` derives Draft 2020-12 validation schemas from the same models used by Python validation. | The prompt, strict provider adapter, example validation, and local validator originate from one Pydantic output model rather than parallel hand-written shapes. |
| [Pydantic strict mode](https://pydantic.dev/docs/validation/latest/concepts/strict_mode/) | Default validation may coerce types; strict models reject values of the wrong type. | Provider output models use strict validation and forbid extra fields. Incidental prose has explicit bounded normalization; authoritative IDs, order, confidence, and numeric choices are never coerced. |
| [OpenAI Structured Outputs](https://developers.openai.com/api/docs/guides/structured-outputs) | A compatible API can accept a `json_schema` response format, but strict support is provider/model specific and supports a constrained schema subset. | Native schema enforcement lives behind the explicit `openai-compatible-json-schema/v1` adapter. The standard adapter stays on JSON-object mode, and both paths require local validation plus role conformance. |
| [OWASP prompt-injection guidance](https://cheatsheetseries.owasp.org/cheatsheets/LLM_Prompt_Injection_Prevention_Cheat_Sheet.html) | Instructions and untrusted data need explicit separation; prompt wording is only one defense layer. | The system prompt names untrusted fields and the user message is one JSON data document. More importantly, the model has no tools or direct write path and can return only a narrow validated draft. |
| [OWASP LLM05: Improper Output Handling](https://genai.owasp.org/llmrisk/llm052025-improper-output-handling/) | Model output must be treated as untrusted before downstream use. | Output is parsed, strictly validated, checked against local identities and bounds, and reconstructed locally before reaching a review surface. It is never executed as code, SQL, a path, or a state mutation. |
| [OWASP SSRF prevention](https://cheatsheetseries.owasp.org/cheatsheets/Server_Side_Request_Forgery_Prevention_Cheat_Sheet.html) | User-configured server URLs need scheme/address validation and redirect controls, especially around private networks. | Provider transport permits only reviewed HTTP behavior, refuses redirects, blocks unsafe destinations by default, and requires an explicit private-network opt-in for a deliberately trusted local provider. |

## Revisit when

- a second provider protocol needs a native structured-output adapter;
- a provider-neutral inference-parameter contract is demonstrated across supported
  services;
- specialized audio input receives its own bounded upload, consent, evaluation, and
  retention contract; or
- measured quality shows that local evidence gates should skip additional model calls.
