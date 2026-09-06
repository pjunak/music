# ADR-025: Bounded shared mood inference

**Status:** Accepted and implemented, 2026-09-06

## Problem

A library run previously selected every stale track. Each request repeated the full
vocabulary, and changing database track IDs changed the system example and schema.
Operational certification fingerprints also served as result identities, making
unrelated implementation or timeout changes invalidate previously paid suggestions.
Tagging and optional manual-tag cleanup exposed independent model configuration.

## Decision

1. Bound each tagging run by tracks, attempted model requests and a conservative
   token reservation. Defaults are 100 tracks, 10 requests and 1,000,000 units.
   Check and checkpoint reservations before I/O. Preserve uncertain attempts; do
   not refund them on timeout or infer missing usage as zero. These controls are
   per-run admission limits, not currency/account budgets.
2. Share one model configuration between `music_tagger` and `tag_cleanup`, while
   retaining independent enablement and strict conformance/quality evaluation.
   Tag cleanup remains local-first and optional; canonical tagging output needs
   no second model pass. Existing divergent assignments require an explicit save
   of Music tagging and cannot silently execute under the old cleanup model.
3. Use stable batch-local slots and a separate untrusted vocabulary reference
   message for native Responses requests. Keep the full vocabulary and validators.
   Select request size against both the existing byte ceiling and output allowance.
   Add documented cache controls only for supported native model families. Report
   cache/reasoning usage as components of the provider totals, never extra charges.
4. Separate result inference identity from runtime certification. The former
   includes rendered task instructions/schema and model/Thinking/output settings;
   source evidence and vocabulary remain independently bound. The latter retains
   reviewed source closures and must pass before new inference. Future semantic
   changes outside the rendered task must bump the inference contract explicitly.
5. Offer opt-in asynchronous native OpenAI Batch with the same strict task and
   review boundary. Schema 12 records the lifecycle before network mutations.
   App limits are 500 requests/32 MiB; no automatic correction or paid retry occurs.
   Only collection is restartable. Collect already-paid results using current
   inference/evidence guards, even when operational certification has expired.

## Batch lifecycle and uncertainty

`uploading -> uploaded -> submitting -> submitted -> provider state -> results_saved
-> terminal` is persisted. A failure between submission and storing its returned ID
requires operator recovery using matching `music_run_id` and input-file identity.
It must never be retried as another paid submission. Explicit abandonment clears
only the local record and cannot cancel unknown remote work.

Known remote files are deleted after durable review-only result storage; cleanup
failures retain `results_saved` for retry. Input expiry is seven days. OpenAI output
retention is up to thirty days. Partial completed results remain useful after
cancellation/expiry and may incur charges. Pending records block role, connection,
credential and master-key mutations between worker runs. Deleting a connection
after terminal completion deletes its Batch records; ordinary job history remains.

## Compatibility and validation

The HTTP additions are Assistant-only; playback/Baton contracts are unchanged.
New request controls default for older clients. Disclosure v12 is required for
new tagging runs. Input v20 and the corrected local spectral implementation expire
older generated results/context once; accepted/manual tags remain unchanged.
No automatic local or provider run is triggered by migration.

Regression coverage includes slot identity, output-aware partitioning, reservation
exhaustion before I/O, quota classification, shared settings and mutation locks,
uncertain submission recovery, owned-ID checks, cancellation, restartable file
cleanup, migration integrity, and explicit frontend actions. Synthetic tests do not
certify a live model, Batch availability, billed cost or musical usefulness.

OpenAI documents a 24-hour completion window and a 50% Batch discount relative to
standard requests. Treat actual model support, cache hits and final billing as
provider observations, not app guarantees. See [Batch](https://developers.openai.com/api/docs/guides/batch),
[prompt caching](https://developers.openai.com/api/docs/guides/prompt-caching), and
[file expiry](https://developers.openai.com/api/reference/resources/files/methods/create).
