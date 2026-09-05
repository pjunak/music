# ADR-019: Model run records and provider attempt outcomes

**Status:** Accepted

**Date:** 2026-09-05

**Scope:** Approved quality-audit implementation, third batch

## Problem

Provider usage was checkpointed only after a request returned. Shutdown or a lost
process during network execution could leave no record of that request. Token
totals also could not distinguish a request known not to have been sent from a
response with missing usage. Each model workflow lacked a common run record.

## Decision

Keep execution in the application layer and use one recorded-request helper for
the four model tools and their four quality evaluations. Before any network I/O,
persist an attempt with an uncertain outcome. If the checkpoint is not acknowledged,
do not send. After transport returns, replace that outcome with the observed facts
and checkpoint again before result validation or proposal persistence.

| Outcome | Meaning |
|---|---|
| `preflight_rejected` | Adapter/request validation rejected execution before sending. |
| `not_sent` | Transport did not enter HTTP execution: for example, unavailable capacity, rejected destination, or a DNS timeout. |
| `response_received` | HTTP response headers arrived, even if status, body size, JSON parsing, or later model validation fails. |
| `uncertain` | An attempt is in progress, HTTP execution failed without a response, or a final observation could not be durably recorded. |

These are transport facts, not billing claims or semantic acceptance. A crash in
the interval between checkpoint and HTTP execution may conservatively retain an
uncertain attempt that was never sent. A failed completion checkpoint may retain
uncertainty even after a response arrived. No remote idempotency guarantee is
invented and no paid job is automatically replayed.

Cancellation after a response still checkpoints known usage under the existing
lease before the job becomes cancelled. Storage failure stops the job lane; normal
startup recovery marks non-restartable jobs interrupted without another request.

## Immutable provenance and budgets

`ModelRunManifest` is constructed before execution and stored in
`result.usage.run_manifest`, including zero-request runs. It records job and role
identity, role/configuration/connection fingerprints, adapter, chosen model and
Thinking mode, timeout, per-request output limit, maximum attempts, evaluation and
disclosure identity, scope/evidence fingerprints, and a typed review destination.
Scope and evidence are hashed locally; the manifest does not copy prompts, paths,
credentials, connection URLs, or private metadata.

EQ and playlist use their prepared logical requests as evidence. Cleanup binds
catalog and vocabulary signatures. Tagging binds the ordered track/source signature
set, including metadata, context, vocabulary, and role identity. Quality evaluations
bind suite/selection identity and the runtime fingerprint covering the synthetic
suite. These hashes support provenance checks; they cannot reconstruct lost inputs.

The maximum attempt count is frozen from task planning. EQ allows one; playlist
allows zero or one; cleanup uses its maximum batch count; tagging uses the actual
adapter-validated batch plan plus its existing two-correction allowance. Evaluation
plans include their required repeats. Execution rejects attempts beyond the frozen
budget and rejects changed role/configuration/connection identity. Output-token
ceilings describe configured request limits, not estimated charges or a billing cap.

Each attempt records a sequence, logical request fingerprint, effective output-token
limit, outcome, and completed transport duration. Retain the latest 128 details and
mark truncation explicitly; lifetime counters and reported tokens cover every
attempt. Keep at most eight reported model IDs, as before. Count incomplete usage
per response, including disjoint missing input/output reports, instead of inferring
that count from aggregate token-report totals.

Generated results still use their existing task-specific validation, stale-evidence
guards, and explicit review/apply paths. A run record is not authorization to apply
or replay anything. Catalog connectors retain their separate evidence-revision and
source-lease contract; a universal model/catalog proposal envelope is deferred.

## Measurement and compatibility

Record queue wait from persisted creation/claim timestamps at one-second resolution;
negative clock differences are unknown. Completed transport durations use a monotonic
clock and milliseconds, excluding queue wait, checkpoints, and local result handling.
Interrupted requests can lack final durations. These measurements are visible in
the existing model usage panel; production percentiles and fairness decisions still
require representative runs behind catalog/evaluation work.

New records use `assistant-provider-usage/v2` and `assistant-model-run/v1`. The browser
continues reading historic usage v1 without inventing outcomes. Their enclosing
background-job result is already extensible JSON; HTTP/WebSocket DTOs and database
schema do not change. Model runtime fingerprints change under the existing policy,
so full conformance/quality checks must be repeated with the chosen configuration.

## Validation and trade-offs

SQLite-backed tests exercise write-ahead visibility before transport, shutdown and
recovery, cancellation after a response, checkpoint failure before commit and after
commit without acknowledgement, completion-checkpoint failure, request-budget
exhaustion, bounded details/model IDs, exact partial usage, role changes, and privacy.
Local HTTP fixtures distinguish DNS timeout, timeout before headers, stalled bodies,
malformed JSON, and HTTP rejection. Browser regressions cover v1 compatibility,
outcome counts, unknown outcomes, incomplete responses, and timing/budget validation.

The extra checkpoint per request adds a small storage cost to preserve accounting.
No transactional outbox or provider retry service is introduced: neither could make
an arbitrary provider request exactly once. Scheduler changes, aggregate latency
studies, live model quality, and physical playback acceptance remain separate work
in [AI_ACCEPTANCE.md](AI_ACCEPTANCE.md).
