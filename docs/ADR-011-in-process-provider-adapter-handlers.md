# ADR-011: Keep provider adapter handlers in process

**Status:** Accepted

**Date:** 2026-08-25

**Amended:** 2026-08-26

**Decider:** Project owner

## Context

Music initially used one OpenAI-compatible request shape for every connection. Real providers can
expose the same `/models` and `/chat/completions` routes while differing in model identifiers,
structured-output parameters, and thinking controls. Google Gemini demonstrates the mismatch: its
model list can return resource names such as `models/gemini-3.7-flash`, completion examples use the
bare model ID, and its OpenAI-compatible thinking control is `reasoning_effort` rather than the
existing `thinking` extension.

OpenAI's current reasoning-model integration is a larger protocol difference, not a parameter
alias: its preferred Responses API uses `/responses`, `instructions` plus `input`,
`max_output_tokens`, nested `reasoning.effort`, `text.format`, and an `output` item list rather than
Chat Completions' messages, `max_tokens`, `response_format`, and `choices` response.

Mature provider libraries such as Pydantic AI and LiteLLM already translate many provider APIs.
They are valuable when they are allowed to own the HTTP client, retries, response models, and model
catalog. Music has a narrower and stricter boundary: user-configured destinations are resolved and
pinned, private destinations are opt-in, redirects are refused, request and response bytes are
bounded, provider work is not retried implicitly, and raw upstream bodies never enter diagnostics.
The feature layer also owns exact prompts, Pydantic validation, identity checks, quality gates, and
review-only results.

## Decision

- Keep network I/O in Music's existing `providers.transport` implementation.
- Add a small registry of versioned, transport-free adapter handlers. A handler may select endpoint
  paths, normalize provider-reported model IDs, choose JSON-object versus strict JSON-Schema response
  mode, and translate the operator's provider-default/on/off choice into documented request fields.
- Select handlers only from the saved adapter ID. Never infer a provider from its connection name,
  URL, or model name.
- Keep the two existing OpenAI-compatible adapter IDs and their behavior stable. They remain the
  broad compatibility path for third-party OpenAI-shaped services.
- Add `openai-responses/v1` for OpenAI itself. Pin `https://api.openai.com/v1`, use the native
  Responses request and response contracts, require the exact task JSON Schema, disable provider
  storage for these stateless task calls, and map Thinking On/Off to nested
  `reasoning.effort=high/none`; Provider default sends no override.
- Add explicit Google Gemini profiles. They pin the documented Google AI Studio base URL,
  canonicalize the `models/` resource prefix, and map thinking On/Off to
  `reasoning_effort=high/none`; Provider default sends no override. Both saved adapter IDs send a
  provider-compatible projection of the exact task JSON Schema because Gemini structured output
  supports only a documented subset of JSON Schema. Unsupported wire constraints such as string
  length, pattern, uniqueness, and exclusive bounds are omitted, and scalar `const` becomes a
  single-value `enum`. The complete generated schema remains in the fixed task prompt and the
  unchanged Pydantic model validates every response locally, so this transport projection does not
  weaken the application contract. The older strict-schema ID remains a compatibility alias rather
  than forcing existing connections to migrate. Send the documented `x-goog-api-client`
  integration-identification header.
- Include handler source in every model-role runtime fingerprint. A handler change invalidates old
  conformance and quality records even when the saved connection and model are unchanged.
- Continue reducing upstream failures to bounded machine-readable codes. Read error JSON within the
  existing byte limit and map only an explicit allowlist of provider code/type/status values; raw
  provider messages, prompts, responses, and credentials remain private. Provider, network,
  timeout, and truncation failures are still never retried implicitly.

## Options considered

### LiteLLM as an embedded execution layer

LiteLLM supports a broad provider catalog and parameter translation. It also introduces a large,
fast-moving dependency surface and owns request execution, optional callbacks, parameter mutation,
and retry behavior. Reproducing Music's DNS pinning, byte limits, no-redirect policy, and exact
no-retry semantics around that layer would be more complex than the provider differences currently
needed. Rejected for direct execution; it remains a possible separately operated gateway reached
through the generic OpenAI-compatible adapter.

### Pydantic AI provider models

Pydantic AI has maintained native providers, structured outputs, model profiles, and custom HTTP
clients. Adopting its model execution would duplicate Music's existing schema harness and move
request/response handling into another abstraction. A custom client alone does not automatically
preserve the current DNS-address pinning and byte-bound response reader. Rejected for direct
execution while the present safety contract remains.

### External multi-provider gateway

An operator can deliberately run OpenRouter, Portkey, LiteLLM Proxy, or another OpenAI-compatible
gateway and configure it as one generic connection. This minimizes application code, but introduces
another trust, retention, routing, credential, and availability boundary. Supported as an operator
choice, not required by Music.

### Small transport-free handler registry

Adds explicit maintenance for each real provider quirk but preserves every existing safety,
validation, disclosure, and review boundary. Selected.

## Consequences

- Gemini and future explicit profiles can be supported without provider branches in playlist,
  tagging, cleanup, or EQ code.
- A provider-side schema projection can express fewer constraints than the canonical task schema;
  strict local validation remains the authoritative fail-closed boundary.
- OpenAI reasoning models no longer receive obsolete Chat Completions fields such as `max_tokens`
  or Music's generic `thinking` extension.
- Adding a native non-OpenAI protocol still requires a deliberate handler and possibly a reviewed
  extension to the shared transport's authentication/header contract.
- Adapter IDs are compatibility contracts. Changed wire semantics require a version bump or an
  intentional fingerprint-invalidating handler change plus fresh conformance and quality runs.
- Mocked tests prove request shaping and safety-boundary preservation, not live provider behavior.
  Each exact provider/model/thinking configuration must still pass conformance and its complete
  synthetic quality gate.

## Sources

- [Pydantic AI provider overview](https://pydantic.dev/docs/ai/models/overview/)
- [Pydantic AI Google provider and custom HTTP client](https://pydantic.dev/docs/ai/models/google/)
- [LiteLLM source and license](https://github.com/BerriAI/litellm)
- [Google Gemini OpenAI compatibility](https://ai.google.dev/gemini-api/docs/openai)
- [Google Gemini structured outputs](https://ai.google.dev/gemini-api/docs/structured-output)
- [Google Gemini partner integration requirements](https://ai.google.dev/gemini-api/docs/partner-integration)
- [Google Gemini API errors](https://ai.google.dev/gemini-api/docs/api-errors)
- [OpenAI latest-model migration guidance](https://developers.openai.com/api/docs/guides/latest-model)
- [OpenAI Responses API reference](https://developers.openai.com/api/reference/resources/responses/methods/create)
