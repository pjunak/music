# AI and playback acceptance after the quality audit

This is the remaining operator validation plan after the fixes described in
[ADR-017](ADR-017-assistant-planning-and-evidence-provenance.md). Automated tests use
synthetic data and local fixtures; they do not establish physical playback or model
quality on a private library.

## Release acceptance

1. Start the candidate against a copy of a representative database. Keep the
   migration's verified backup. Confirm schema 10, preserved manual tags, expired
   legacy catalog proposals, and successful new proposal generation.
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

## Provider acceptance

Use the operator's selected connection, model, and Thinking setting. Re-run role
conformance and the complete quality suite after deploying changed runtime code.
Retests of failed cases remain diagnostic and cannot replace full certification.
Do not lower thresholds or remove required concepts to make a model pass.

Review the disclosed payload and planned request count before any live-library
run. Verify a small scope first, then force-rebuild that same scope and confirm the
server estimate changes appropriately. Include vocabulary sizes near the payload
limit and verify a rejected oversized plan causes no provider request. Preserve the
two-correction limit and record usage for unsuccessful attempts as well as successes.

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

- Derive static output-schema structure from strict result types. Keep dynamic
  track/tag identifier sets, numeric limits, and local reconstruction authoritative.
  Prove schema/validator agreement with adversarial fixtures before switching.
- Extract catalog connector policy and mapping orchestration behind typed
  application ports, following the model-job boundary now in place.
- Measure queue delay for small interactive drafts behind long catalog/evaluation
  jobs before changing provider-lane fairness. Preserve per-provider request limits,
  cancellation, checkpointing, and non-restartable paid jobs.
- Make attempt outcomes distinguish preflight rejection, known-unsent requests,
  responses received, and uncertain external outcomes. Do not treat a timeout as
  proof that the provider performed no work.

These are follow-up changes and validation tasks. The current synthetic tests do
not support claims about physical audio quality, private-corpus tagging accuracy,
or production latency.
