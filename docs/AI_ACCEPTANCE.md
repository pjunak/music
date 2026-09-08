# AI and playback acceptance after the quality audit

This records acceptance and the remaining validation plan, with responsibilities split below, after the fixes described in
[ADR-017](ADR-017-assistant-planning-and-evidence-provenance.md). Automated tests use
synthetic data and local fixtures; they do not establish physical playback or model
quality on a private library.

## Current use of this record

Provider/model/Thinking configurations are swappable in the server UI. The dated
results below are observations of particular configurations, not a settled choice
of production models. Preserve the evidence while evaluating replacements against
their current conformance and quality contracts. Astra may be used as the coding
agent to build and tighten the custom harness; that does not certify an application
role or authorize paid requests or private-library disclosure.

Environment limitations in checkpoints describe that run only. Check current
tool and host availability before declaring a task blocked; record what was
actually verified and keep local engineering, provider and physical acceptance
separate.

## Operator checkpoint: 2026-09-09

The supplied Sol (`gpt-5.6-sol`, Thinking disabled) export for tagging suite v22
passed conformance and 59/63 quality scenarios. All 13 safety scenarios passed;
bundled vocabulary was 53/57, custom 5/5 and maximum vocabulary 1/1. The independent
context-only score was 8/9, below 90%, so certification correctly failed under
that declared threshold. The export reports nine requests, 93,378 input tokens
and 5,265 output tokens; these are observations of this run, not a price estimate.

Two fixture expectations were underspecified: a puzzle setting did not establish
an inquisitive musical mood, and heavy battle music did not explicitly establish
suspense. Suite v23 adds the missing descriptive genre evidence while retaining
the required and forbidden tags. The castle omission remains a valid semantic
miss. The loud settled-texture abstention identifies a gap in the task guidance:
input v22 explains that recording level contributes 50% of window intensity, so
these correlated values must not outweigh unchanged texture and development.
The old export discarded model evidence, so the exact reason for its abstention
cannot be recovered. New quality results retain that bounded public explanation.

The badge now counts distinct scenarios before and after completion; thirteen
safety reruns are included in those scenarios rather than increasing its total
from 63 to 76. Detailed logs retain the individual-check count. Neither the
90% gates nor strict safety validation were relaxed. Keep Sol with Thinking off
for the next deliberate full check after installation; this engineering change
has not been evaluated against the paid provider and does not establish a pass.
No saved configuration, live library, provider quota or deployment was changed.

Local validation passed: 390 Rust tests, 306 frontend tests (plus the focused
35-test rerun after shortening the badge text), workspace check, strict Clippy,
formatting, documentation tests, generated-contract checks, and frontend
lint/typecheck/build. Architecture and listening-pilot tooling passed 11 Node
tests. An offline browser fixture verified the running, safety-rerun and completed
states, including the compact badge and failure evidence. None of these checks
certifies Sol on the revised suite or establishes listening accuracy.

## Operator checkpoint: 2026-09-05

Engineering cleanup is complete. The final acceptance follow-up repairs a
demonstrated playlist input omission; it adds no new AI capability. The operator
reported working web and Baton playback and working provider requests. These
observations close basic playback/connectivity acceptance, without implying every
SFX, reconnect, migration or proxy action below was exercised.

| Tool | Evidence supplied | Status and next action |
|---|---|---|
| Mood tagging | Luna (`gpt-5.6-luna`), Thinking off: full v21 suite **53/56 passed**. Bundled **47/50**, custom **5/5**, maximum vocabulary **1/1**. | Passed the declared gate with three nonblocking misses. Preserve them for the later usefulness study; passing does not mean flawless tagging. |
| Mood-tag cleanup | Operator reports excellent results with DeepSeek Flash; no cleanup export supplied. | Working by operator observation. Do not turn that observation into a quantified accuracy claim. |
| Playlist planning | Terra (`gpt-5.6-terra`), Thinking on: full v6 suite **12/14**, quality **failed** despite successful request execution. | Both missed tracks were present in the candidate pool. The harness omitted declared alias/context-cue meanings. Input v4 now supplies those meanings; the unchanged full suite must be rerun on this runtime. Offline tests do not establish a Terra pass. |
| EQ assistance | Operator explicitly deferred testing because the current ten-band scope covers too few desired controls. | Evaluation and expanded controls are deferred. No EQ acceptance is claimed or required for closing this cleanup phase. |

The Luna misses were `medieval-tavern-dance` (missing medieval),
`castle-records-ambiguity` (initial calm abstention/low confidence; repeat returned
calm), and `slow-tempo-high-intensity-siege` (missing tense). The castle scenario's
safety label does not make a semantic miss blocking; forbidden false positives
and contract failures still block certification. No gate was relaxed.

The playlist repair advances its input to v4 and disclosure to v3. Changing the
shared role-contract inventory conservatively invalidates saved gates for all
roles. After installing this revision, rerun conformance and the complete quality
suite for each role that will be used, retaining its exact chosen model/Thinking.
Leave EQ deferred. Historical exports remain valid evidence of their original
runtime; the server requires fresh matching gates before live work. No paid
requests, saved role edits or deployment were performed by this follow-up.

## Who does what

| Work | Codex owns | Operator owns |
|---|---|---|
| Code fixes and cleanup | Implementation, regression tests, documentation, local gates, commits, and a clear remaining list. | Product preferences when a real trade-off needs a decision. No coding required. |
| Docker/release checks | Build and run the verification script when a Docker host is accessible; diagnose and fix failures. | Provide the host/access or run the supplied commands there. Approve production deployment separately. Docker was unavailable in the recorded audit environment; recheck current availability. |
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
   migration's verified backup. Confirm schema 12, preserved manual tags, expired
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
new model certification. The dated checkpoint above records supplied results and
which runtime changes require renewed certification before using those roles.
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

## Next phase: evaluate usefulness

After the corrected playlist run, assess the approach before adding AI features.
Codex can compare local-only and model-assisted results on the same permitted
sample, separate retrieval, harness and semantic errors, and report latency,
reported tokens, correction attempts and operator correction time. The operator
provides independent scene/tag judgments and decides whether the saved effort is
worth the cost and complexity. Provider-reported usage is not a portable price;
use actual billing evidence for cost comparisons.

Agree on useful outcomes first: mood tags should help find suitable music,
cleanup should reduce duplicate labels without wrong merges, and model playlists
should improve selection enough to justify another request and review. Test
metadata-only versus current local context where authorized. Keep EQ outside this
study until its scope is deliberately revisited. This study is the next task, not
unfinished engineering cleanup.

### Held-out quality study

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

These are conditional follow-ups. Do not implement scheduling changes or tune
recall without a demonstrated need. Synthetic tests do not establish private-corpus
tagging accuracy, physical audio quality or production latency; basic playback is
separately confirmed by the operator above.

The static output schemas and typed catalog connector boundary are implemented in
[ADR-018](ADR-018-derived-model-schemas-and-catalog-ports.md). Their automated checks
cover strict result handling and SQLite-backed orchestration; consult the dated
checkpoint for actual provider and physical observations and their limits.

[ADR-019](ADR-019-model-run-records-and-attempt-outcomes.md) documents implemented
run manifests, attempt accounting, fault recovery, and measurement limits. The
operator-supplied provider exports are separate evidence from those local tests.

## Mood efficiency acceptance (2026-09-06)

Engineering now supplies bounded runs, shared mood configuration, Batch recovery, accounting,
and corrected spectral coverage. No live provider call was used to validate this implementation.
The operator's remaining work is to save the shared model, pass both relevant task suites,
refresh a small sample's local context, and run a bounded Batch pilot (for example 20 tracks,
2 requests, 150,000 reservation units; use the actual preview if it requires a smaller scope).
Verify provider acceptance, reported usage, collection after restart and review-only suggestions.
Do not interpret a tagging pass as a cleanup pass: the supplied Luna cleanup export previously
failed one of twenty strict cases. Keep cleanup unavailable until the selected shared model passes.
Then assess useful tags by listening, using [the context review](MOOD_CONTEXT_REVIEW.md).
