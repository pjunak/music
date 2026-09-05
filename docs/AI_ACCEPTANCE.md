# AI and playback acceptance after the quality audit

This is the remaining operator validation plan after the fixes described in
[ADR-017](ADR-017-assistant-planning-and-evidence-provenance.md). Automated tests use
synthetic data and local fixtures; they do not establish physical playback or model
quality on a private library.

## Release acceptance

1. Start the candidate against a copy of a representative database. Keep the
   migration's verified backup. Confirm schema 10, preserved manual tags, expired
   legacy catalog proposals, and successful new proposal generation. The typed
   connector update changes catalog evidence signatures again; old generated
   proposals become stale while accepted/manual tags remain unchanged.
2. With a physical Android phone selected, play music and overlapping SFX. Select
   another output from the browser. Both phone lanes must stop; newly fired SFX
   must remain silent. Select the phone again and confirm normal playback.
3. Disconnect the phone, change the output selection elsewhere, and reconnect.
   Baton must reconcile the new snapshot without replacing the other output set.
   Separately verify the server-owned output-by-default designation on registration.
4. Start a catalog lookup, attempt to edit its source or credential, and verify the
   explicit busy response. Cancel the lookup, then apply the edit. Rename a
   vocabulary tag and ensure old proposals cannot be accepted, including after a
   fresh lookup regenerates a tag with the same name.
5. Build the release image on a Docker host and run the existing image verification
   script. Exercise authenticated web/Baton registration, reconnect, transport,
   queue, device selection, and output volume against that image.
6. Generate a small current model-tag scope, open its pending review list, accept
   one explicit suggestion, reject another, and reopen a decision. Accepted manual
   tags must survive rejection/reopening. Change a vocabulary definition or model
   setting and verify old proposals disappear and cannot be accepted from an old
   page. [ADR-021](ADR-021-current-model-tag-review.md) records the restored review
   path and its atomic stale-result checks.

## Provider acceptance

Use the operator's selected connection, model, and Thinking setting. Re-run role
conformance and the complete quality suite after deploying changed runtime code.
Retests of failed cases remain diagnostic and cannot replace full certification.
Do not lower thresholds or remove required concepts to make a model pass.

Tagging now evaluates the bundled, custom, and 200-tag vocabularies separately;
each group must meet the same 90% threshold and avoid blocking failures. Inspect
the per-group summary when a high overall score still fails. Playlist reports
identify relevant fixture tracks omitted from the candidate pool before ranking,
including when a provider fails. Evaluate those local omissions separately from
incorrect model selections. Both changes are described in
[ADR-020](ADR-020-vocabulary-quality-and-candidate-recall.md).

Review the disclosed payload and planned request count before any live-library
run. Verify a small scope first, then force-rebuild that same scope and confirm the
server estimate changes appropriately. Include vocabulary sizes near the payload
limit and verify a rejected oversized plan causes no provider request. Preserve the
two-correction limit and record usage for unsuccessful attempts as well as successes.

Usage v2 retains a shared run manifest and write-ahead attempt outcomes. Check that
the chosen model/Thinking, request limit, scope/evidence fingerprints, and review
destination match the run. A cancelled/interrupted attempt can remain uncertain;
zero reported tokens do not establish zero charges. Compare any uncertain attempt
with provider-side records before deliberately starting another paid run.

## Held-out quality study

Create a versioned set of 100–200 independently labelled, permitted examples covering
ambiguous metadata, misleading titles, mixed genres, sparse metadata, and incomplete
local context. Keep titles and paths out of the provider payload. Store labels
separately from prompts; do not tune prompts against the held-out split.

For each fixed provider/model/Thinking configuration, record vocabulary and role
fingerprints, per-tag precision/recall, unsupported-specificity errors, abstention
rate, accepted/rejected suggestions, schema failures, correction requests, request
cost, and latency. Repeat safety examples and run the same candidate configuration
more than once to expose variation. Report results by scenario and tag group before
changing prompts, mappings, or model recommendations. Human labels and permission
to send the selected metadata are prerequisites, not outputs to invent locally.

## Engineering follow-up

- Shared immutable manifests and request budgets are implemented for all model
  tools/evaluations. Assess a common model/catalog proposal provenance envelope
  only if it improves review workflows; retain catalog revision and lease guards.
- Custom and maximum-vocabulary tagging and cleanup cases, model-tag review, and
  separate playlist candidate recall are implemented. Record operator review
  metrics without using decisions as automatic training labels. Use
  candidate recall measurements to justify any retrieval-policy changes.
- Collect queue-wait and completed-request-duration measurements from usage v2
  for small interactive drafts behind long catalog/evaluation jobs. Compare queue
  delay percentiles by job type and workload before changing provider-lane fairness.
  Queue timestamps resolve to seconds; interrupted durations may be unavailable.
  Preserve request limits, cancellation, checkpointing, and non-restartable paid jobs.

These are follow-up changes and validation tasks. The current synthetic tests do
not support claims about physical audio quality, private-corpus tagging accuracy,
or production latency.

The static output schemas and typed catalog connector boundary are implemented in
[ADR-018](ADR-018-derived-model-schemas-and-catalog-ports.md). Their automated checks
cover strict result handling and SQLite-backed orchestration; provider and physical
acceptance above still need the actual configured runtime.

[ADR-019](ADR-019-model-run-records-and-attempt-outcomes.md) documents implemented
run manifests, attempt accounting, fault recovery, and measurement limits. No live
provider, production-latency, or private-corpus result is implied by these tests.
