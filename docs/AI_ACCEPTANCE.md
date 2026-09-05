# AI and playback acceptance after the quality audit

This is the remaining validation plan, with responsibilities split below, after the fixes described in
[ADR-017](ADR-017-assistant-planning-and-evidence-provenance.md). Automated tests use
synthetic data and local fixtures; they do not establish physical playback or model
quality on a private library.

## Who does what

| Work | Codex owns | Operator owns |
|---|---|---|
| Code fixes and cleanup | Implementation, regression tests, documentation, local gates, commits, and a clear remaining list. | Product preferences when a real trade-off needs a decision. No coding required. |
| Docker/release checks | Build and run the verification script when a Docker host is accessible; diagnose and fix failures. | Provide the host/access or run the supplied commands there. Approve production deployment separately. Docker is unavailable in the current workspace environment. |
| Representative database migration | Prepare the isolated test, run it on an approved copy, compare preserved data, and diagnose failures. | Supply or authorize the particular representative copy. Keep production untouched until acceptance. |
| Phone and speaker acceptance | Prepare exact actions, inspect logs, and fix failures. | Operate the physical phone and confirm what actually plays or stops. |
| Chosen-model tests | Run and analyze the existing full suites once the environment and disclosed run are authorized; fix harness defects without lowering the gates. | Keep the chosen provider/model/Thinking, authorize the disclosed provider requests and any private metadata scope. |
| Suggestion usefulness | Prepare a small review set and summarize corrections and failure patterns. | Judge whether tags, playlist choices, and EQ suggestions are useful for the intended scenes. Codex must not invent independent human labels. |

Feature development can resume once the approved fixes and bounded cleanup are
implemented and the local engineering gates pass. That work now includes session
hardening, pure tag-review planning, separated import and library-cleanup helpers,
and explicit stylesheet ownership. Further extraction should follow actual changes.

Release acceptance and AI-role acceptance remain separate: record the applicable
deployment/physical checks below, and require each role being enabled to pass its
current full suite and a small review-only trial. Codex owns preparation and fixes;
operator access, consent and independent observations remain necessary. A larger
research corpus, scheduler redesign, or new feature is not a prerequisite for
resuming development. [TODO.md](../TODO.md) contains conditional and future work.

## Release acceptance

1. Start the candidate against a copy of a representative database. Keep the
   migration's verified backup. Confirm schema 11, preserved manual tags, expired
   legacy catalog proposals, and successful new proposal generation. The typed
   connector update changes catalog evidence signatures again; old generated
   proposals become stale while accepted/manual tags remain unchanged.
   Schema 11 revokes legacy logins once; sign in again on the browser and Baton.
   Confirm Settings identifies the current session, can revoke another session,
   and leaves the current one usable. Accounts and authored data must survive.
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
   Confirm same-origin browser sockets connect through the actual proxy and Baton
   reconnects after sign-in. If the proxy rewrites Host, configure the public
   browser origin in `ALLOWED_ORIGINS`; do not trust forwarded headers implicitly.
6. Generate a small current model-tag scope, open its pending review list, accept
   one explicit suggestion, reject another, and reopen a decision. Accepted manual
   tags must survive rejection/reopening. Change a vocabulary definition or model
   setting and verify old proposals disappear and cannot be accepted from an old
   page. [ADR-021](ADR-021-current-model-tag-review.md) records the restored review
   path and its atomic stale-result checks.

## Provider acceptance

Use the operator's selected connection, model, and Thinking setting. Run role
conformance and the complete quality suite for the current role fingerprint.
Changes to the provider, harness, task contract, or relevant vocabulary can make
earlier results stale; unrelated authentication changes do not themselves require
new model certification. Pending tests from the earlier Assistant changes still
need to be completed before enabling those roles.
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

Start with a small permitted sample that the operator can review meaningfully.
Expand to a versioned set of 100–200 independently labelled examples if stronger
quality comparisons are needed; the larger study is optional. Cover
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
- Custom and maximum-vocabulary tagging and cleanup cases, model-tag review and
  current operator review metrics are implemented. Decisions are not automatic
  training labels. Playlist vocabulary recall repairs measured misses in controlled
  synthetic fixtures; use `evaluate-playlists SUITE --engine candidates --json` to
  measure candidate availability without a provider. Assess gains and displaced
  candidates on the independently labelled study before tuning the recall quota.
- Use `music-cli jobs timing --database /data/app.db --limit 1000 --json` for
  read-only queue-wait and whole-job execution percentiles by lane and kind.
  Pair these with completed-request durations from model usage v2 for short drafts
  behind long catalog/evaluation jobs. The same-second UUID ordering defect is
  repaired; the provider lane still executes one whole job at a time. Check sample
  coverage, queued/running counts, and unavailable durations before comparing
  percentiles. Follow the [measurement and scheduling plan](JOB_DIAGNOSTICS.md)
  before changing fairness, request limits, cancellation, or paid-job restart policy.

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
