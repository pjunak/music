# Song evidence and mood tagging implementation plan

Prepared 23 September 2026; clean-cutover scope reviewed against `music` commit `046f67e`.
Status: core pipeline implemented; Jev quality repair, independent listening and production acceptance remain open. No model is certified.
This turns the [research](SONG_EVIDENCE_RESEARCH.md) into dependency-ordered work.
Public model specifications and current source were inspected; native compatibility,
listening accuracy, licensing suitability, and production cost still need the gates below.
The status below identifies delivered contracts; conditional stages remain proposals.

## Jev evidence inputs experiment — 28 September 2026

Prepared a controlled data experiment with 14 variants, unchanged questions and
all current synthetic cases. It separates source omissions/combinations, acoustic
detail, numeric versus named bands, reversible compaction, repeated detail and
source-attribution probes. Six identical-input controls expose variation. No-op
and empty variants are skipped explicitly, and the report preserves incomplete
pairs, per-group/source coverage and actual usage without selecting a winner.
Exact provider bodies are deduplicated across cases/arms, saving 49 calls while
preserving all case expectations. Shared answers cannot masquerade as independent
observations, and total usage counts each physical response once.

The complete offline plan is **1,616 requests / 82,194,553 conservative input
units**. It needs a separate paid-run budget; no new Jev result is claimed.
The [acceptance record](AI_ACCEPTANCE.md#jev-evidence-source-and-amount-experiment-prepared-2026-09-28)
contains the exact hash, matrix, source limitations and conservative cost estimate.
The current suite has only three mixed text/audio cases and no catalog cards;
synthetic source relocation is not real catalog validation. Production v9 stays
unchanged. Overall accepted progress remains approximately **80%** until quality,
independent listening and production acceptance are complete.

Local checks pass: **12** experiment tests, including shared-answer expectation and
usage accounting, incomplete responses, exact approval and period-report regressions;
strict example Clippy, formatting, seven architecture policy tests and the actual
architecture check. The refactored journal reader reproduces the preceding live
criteria report byte-for-byte. All **58** local links in the changed documents resolve.

## Jev criteria experiment and regression coverage — 28 September 2026

**Paid comparison outcome: rejected.** All 366 approved requests completed with
3,973,016 input and 311,830 output tokens, no retries or unresolved attempts.
Removing optional initial-fit criteria recovered no required tags, lost four, and
introduced one forbidden custom candidate before grounding. Required first-pass
assignments fell from **139/151 to 135/151**; this is not a full-suite pass rate.
V9 keeps explicit criteria, and no full validation of this candidate is warranted.
See the [dated results](AI_ACCEPTANCE.md#jev-optional-criteria-comparison-rejected-2026-09-28).

Implemented the documented optional-criteria form of Noul in the typed adapter.
Missing criteria are omitted from requests; incomplete explicit criteria and empty
Choice options remain invalid. Existing v9 requests are unchanged.

Replaced the rejected fifteen-case experiment with one controlled comparison over
every current synthetic case and vocabulary tag. It changes only initial-fit
criteria, preserving actual production partitions and all evidence/instructions.
Previously passing positives, including every v8 regression, now participate.
An offline journal report separates gains, lost positives, new forbidden candidates
and unscored changes, and exposes incomplete pairs and missing usage. It cannot
certify a model or change application acceptance. No new dependency or audio model
was added. The separately approved run reproduced the exact prepared plan hash and
17,016,148-unit cap; all eight runner tests passed before execution.

The semantic audit confirms useful descriptive support for Arctic, temple,
festival and storytelling, while city/shopping, court festivities and synthetic
acoustic-to-mood expectations retain genuine ambiguity. Do not optimize wording
solely to push boundary probes over 0.70. Vocabulary, fixtures and thresholds stay
unchanged; independent listening is still needed to qualify the audio assumptions.

Next: audit the propositions/evidence cards behind literal metadata misses before
preparing another controlled comparison. Preserve regression and negative controls;
keep ambiguous expectations and independent acoustic listening as separate work.
Do not adopt criteria removal, lower thresholds or rerun this rejected candidate.
Overall accepted-delivery progress remains approximately **80%**; improved
investigation evidence is not a new quality acceptance. No fresh model certification
or production rollout is claimed.

## Jev setting/activity repair — 28 September 2026

**Full-run outcome: rejected.** V8 reached **49/66**, down from v7's 55/66, and
retained **140/159** required assignments instead of 148. All nineteen omissions
failed initial matching; safety stayed 16/16, custom 5/5 and acoustic/context-only
6/9. The limited diagnostic missed regressions in previously passing positives.
**V9 restores the v7 questions**, retains broader fingerprint coverage and reviewed
budget enforcement, and adds no fallback. No thresholds or fixtures were relaxed.
The approved run used 287 requests, 3,358,024 input and 214,363 output tokens with no
retries. This is not a fresh v9 certification. See the
[full record](AI_ACCEPTANCE.md#jev-full-v8-validation-and-rollback-2026-09-28).

The owner's new v7/v29 report confirms **55/66** with exactly the same eleven
initial-match omissions as the preceding local run. Grounding, custom vocabulary
and all safety checks retain their improvements. A bounded experiment separated
setting, activity and mood predicates: v7's shared first-pass criteria ask
setting/activity questions to satisfy an emotion-oriented rule.

The authorized comparison used fifteen synthetic cases with paired controls,
including negative controls, unchanged evidence/definitions/scopes/conflicts and
unchanged custom predicates. All 28 requests completed: 119,043 input and 5,896
output tokens, within 469,750 reserved units. Setting/activity fit improved or held
in all six target cases, with Arctic and temple reaching 0.70. Metadata support
independently improved or held. The mood rewrite worsened acoustic calm and urgency.

Candidate **v8** used only the measured setting/activity first-pass and album/genre
support changes. All other judgment families and thresholds remain; no legacy
fallback, audio feature or dependency was added. Inference identity now covers every
group-specific question family. Focused regressions passed. The unchanged v29 full
suite was authorized at 818 requests / 20,172,234 conservative units and rejected the
candidate as recorded above. Future comparisons must cover previously passing
positives alongside failures and negative controls.

All **581 Rust tests** and the backend gates pass, plus four example tests. An
offline payload comparison confirms exactly 33 measured question replacements,
113 unchanged questions and unchanged evidence across all fifteen selected cases.

The [acceptance investigation](AI_ACCEPTANCE.md#jev-first-pass-investigation-2026-09-28)
records all eleven scores and distinguishes the question mismatch from ambiguous
event-to-emotion and unqualified acoustic expectations. Overall accepted-delivery
progress remains approximately **80%** until quality and independent listening pass.

## Jev descriptive support and fixture repair — 28 September 2026

The latest supplied v6/v28 run reached **45/66**, with **16/16** safety and
**6/9** context-only cases. Twenty-two required assignments were missing (six
grounding, sixteen matching), plus one unsupported extra custom tag.

Implemented engine **v7** using a separately authorized thirty-call comparison.
Only album/genre support adopts the measured direct-semantic question: explicit
descriptions can support their meaning, while a compatible attribute alone cannot
establish a required purpose such as focused study. First-pass, acoustic/catalog,
conflict and threshold behavior remains unchanged because the broader candidate
rewrite worsened several judgments. No dependency, heuristic fallback or new
audio model was added.

Suite **v29** corrects four positive setting inputs whose only specific evidence
was excluded source provenance. Their required/forbidden tags are unchanged;
paired regressions prove that source and artist identities alone remain unusable.
The quality registry now references the canonical suite constant. The bounded
developer tool compares both fit and selected support with negative controls;
previous experiment bodies remain in dated local journals.

Next, in programming order:

1. Investigate initial scene/setting and mood semantics using fixed development cases;
   do not generalize a successful support prompt to other decisions without evidence.
2. Qualify the three audio-only expectations through independent listening and
   evaluate additional acoustic evidence only against an identified useful gap.
3. Require a full quality pass, then the independent real-music and production
   resource/playback acceptance checks before claiming completion.

Completed full v7/v29 validation: **55/66 (83.3%)**, **16/16 safety**, **5/5 custom**,
**1/1 maximum vocabulary**, **6/9 context-only**, **148/159 required assignments**.
Ten scenarios improved, none regressed. All eleven remaining required-tag misses
are initial-match rejections; no required tag is now lost at grounding. The model
remains uncertified. Provider usage was 289 requests, 3,258,585 input and 216,690
output tokens, with no retries or uncertainty. All 580 Rust tests and required
backend gates pass; the four developer-example tests also pass.

See the [dated comparison and validation](AI_ACCEPTANCE.md#jev-metadata-support-repair-2026-09-28).
Overall accepted-delivery progress remains approximately **80%**; a narrower
harness repair is useful but does not close the quality or listening gates.

## Measured Jev framing repair — 27 September 2026

The latest supplied v5/v28 export contains 15/66 passing scenarios, not 16. All
269 requests completed and all 16 safety checks passed. It returned 59/159 required
assignments, including only 6/70 moods. Ninety-four misses failed initial matching,
three failed later period applicability and three failed grounding.

Completed the authorized 30-request synthetic comparison, then implemented its
measured literal-question/neutral-card combination as engine v6. Metadata meaning
is judged separately from independent recording verification. Source attribution,
tentative support, observation grounding, eight-candidate limits and all 0.70 gates
remain. No old-engine fallback, automatic tag assignment or additional audio model
was added. The console now separates passed scenarios from progress and totals
missing required assignments by stage, including the period follow-up.

The separately authorized fresh conformance and unchanged full v28 suite completed:
**44/66 scenarios, 16/16 safety, 135/159 required assignments and 60/70 moods**.
Custom vocabulary remains 4/5 and context-only remains 6/9. Thirty scenarios improved;
one custom case regressed by returning an extra tag. The quality gate still fails.
The full run used 289 requests and 3,258,025 reported input tokens, within the
818-request/20-million conservative-unit cap, with no retry or uncertain request.
The [acceptance record](AI_ACCEPTANCE.md#jev-controlled-comparison-and-full-validation-2026-09-27)
and [ADR](ADR-026-native-jev-evidence-judgments.md#controlled-framing-repair-engine-v6)
preserve the results and distinguish confirmed defects from remaining hypotheses.

The bounded developer example generates reviewable offline plans; paid runs use
explicit hash/budget arguments and fresh journals. Full checks reuse the production
planner/executor, durable attempts and scorer in an isolated local database, with
no application acceptance writes. Adopted duplicate variants are omitted. This is
development tooling, not a new production service, runtime or dataset platform.

Next, in programming order:

1. Correct four positive fixtures whose setting exists only in provenance, retaining
   the same required tags and separate provenance-only negatives. Record a new suite
   version; never retroactively rescore this unchanged v28 run as passing.
2. Compare narrowly defined observation-support questions on development cases,
   including explicit descriptors, contradictory evidence, complete custom criteria
   and negative controls. Nine current required assignments pass matching but fail
   grounding; another eight initial misses concern use/mood semantics. Further paid
   tests need a separately reviewed plan.
3. Qualify the three audio-only positive misses with independent listening and
   useful upstream evidence. Do not force proxy-to-emotion rules or fabricate stronger
   measurements to pass the suite.
4. Re-run fresh conformance and the complete fixed suite after a measured repair,
   then validate the real-music pilot and production resource/playback behavior.

Validation: 579 Rust tests pass without skips, with real FFmpeg and the pinned
optional voice model enabled; 380 frontend tests pass. Workspace check, strict
workspace/fuzz Clippy, formatting, architecture, doc tests, generated contracts,
frontend lint/typecheck/build and developer-example checks pass. No push or deployment
occurred. Overall accepted-delivery progress remains approximately **80%**: the
measured improvement is substantial, but synthetic quality and independent listening
are still open gates.

## Jev evidence views and rejection diagnostics — 27 September 2026

The owner's completed v4/v27 run passes 55/65 scenarios, with 14/15 safety,
4/5 custom-vocabulary and 6/9 context-only passes. All 418 requests received
responses, reporting 5,081,125 input tokens. The remaining issues concern evidence
and semantics, not another transport abort; this result does not measure listening
accuracy.

Implemented engine v5 with definitions/synonyms instead of display names and
retrieval associations. Mood, use and period questions have consistent, separate
criteria. Musical questions exclude identity/provenance, absolute recording level
and non-supporting technical fields; custom definitions retain their factual input
view. Coverage qualifies evidence and measurement reliability stays beside acoustic
observations, including sections and endings. Original facts stay in local profiles.
Questions share requests only within the same view, with conservative group-specific
worst-case reservations and the existing eight-candidate bound.

Quality report v10 adds bounded request states and every tag's fit, period score,
candidate status, observation support/conflict scores and final/pending decision
stage. Required tags are attached after execution. The existing test console
summarizes failures and its JSON export includes all primary/repeat traces. Live
song jobs do not persist these diagnostic traces. No new store, model, legacy
reader, automatic retry or external comparison service is added.

Shared input v25 clarifies that Origin is provenance. Suite v28 supplies an explicit
castle-use album description and a separate provenance-name-only negative case:
66 scenarios and 16 repeats. All previous required tags, the 90% thresholds, custom/
200-tag coverage and nine-case context-only gate remain. The urgent fixture's prose
now describes the supplied proxies rather than a withheld pulse. Its expected label
and measurements remain unchanged. Independent listening must qualify these acoustic
positives; no stronger measurements or listening judgments were invented.

Validation: all 579 Rust workspace tests pass without skips, with real FFmpeg and
the pinned optional voice model enabled; all 377 frontend tests pass. Workspace
check, strict workspace/fuzz Clippy, formatting, architecture, doc tests, generated
contracts, frontend lint/typecheck/build and 139 local documentation links pass.
The 17 native-engine regressions include label/cue invariance, gain-only request
identity, preserved endings/reliability, scoped custom facts, rejection stages,
period gates, separate primary/repeat traces and worst-case request budgets.
Durable execution tests also retain partial diagnostics on a rejected response.
A mechanical comparison confirms all 65 original scenarios retain every required/
forbidden tag, maximum, support constraint and gate.

Fresh pinned Jev conformance, full v28 quality and independent listening remain
required. Live tag-quality benefit and token savings are unmeasured; the acoustic
positives still need independent listening qualification. No paid calls, push or
deployment occurred. Overall accepted-delivery progress remains approximately **80%**.

## Jev response compatibility — 27 September 2026

Continued the native API audit after the v4 repair. The documented SDK allows
omitted/null token counts and ignores additional response fields; our parser
rejected both as `typed_response_shape_invalid`. Dedicated regressions reproduced
both failures before the fix. Counts now remain optional, and extra fields are
discarded when validated answers are reconstructed. Missing question IDs, required
fields, invalid scores, malformed reported counts and model mismatches still fail.

Durable accounting retains the full reservation and explicitly marks missing usage,
including on partially failed runs. The existing usage panel reports incomplete
totals. There is no new UI, store, provider SDK, legacy path or automatic retry.
Engine v4, suite v27, all prompts and all quality thresholds remain unchanged; the
runtime fingerprint requires fresh acceptance after this adapter change.

Validation: both new parser regressions failed before the fix; all 31 focused
native tests and all 573 Rust workspace tests now pass, with no skips in the full
suite. Real FFmpeg and the pinned optional voice model were enabled. Workspace
check, strict workspace/fuzz Clippy, formatting, architecture, doc tests, generated
contracts and 215 local documentation links pass. No new live provider result is
available in this batch. Overall progress remains approximately **80%**; full Jev
quality and independent listening remain the outstanding acceptance gates.

## Jev quality abort and evidence-contract repair — 27 September 2026

The latest owner export reached 19 primary cases before our exact-sum Choice parser
rejected case 20. It then skipped 43 primary cases and all 13 safety repeats.
The TypeSafe SDK documents approximate sums without a rounding bound. Engine v4
removes that invented restriction while preserving raw scores, finite/range checks,
exact options and a positive maximal winner. No normalization, lowered Noul gates
or automatic retries are introduced.

Applicability questions now consistently ask for positive evidence for a tentative
musical impression or tabletop use. Descriptive metadata can contribute without
proving the sound; isolated identity words and commands cannot. Shared suite v27
supplies missing evidence in seven ambiguous positives and adds two safety pairs:
Arctic geography cannot establish cold emotion, and a castle march cannot establish
heroism. All original required tags, the 90% requirements, nine context-only cases,
custom/200-tag coverage and strict safety repeats remain. There are now 65 scenarios
and 15 repeats. No provider-specific easier certification is introduced.

See [the updated decision record](ADR-026-native-jev-evidence-judgments.md) for the
confirmed failure, revised evidence contract and official references. Existing
analysis and accepted tags remain usable. Deployment must be followed by fresh
conformance and the complete quality suite. Live Jev quality and independent
listening are still unverified; overall progress remains approximately **80%**.

Validation: all 570 Rust workspace tests pass with no skips, including the real
FFmpeg and pinned optional voice-model fixtures. The 31 focused regressions cover
rounded Choice totals through parsing, native HTTP and durable jobs; threshold
preservation; strict invalid-answer checks; complete vocabulary and budget bounds;
and blocking failures for the two new safety pairs. Workspace check, strict
workspace/fuzz Clippy, formatting, architecture, doc tests, generated contracts and
local documentation links pass. Frontend and wire shapes are unchanged. No paid
provider calls, push or deployment were performed; live quality is not certified.

## Dedicated Jev evidence judgments — 27 September 2026

The next owner export confirms conformance passed for pinned Jev 1.13.0, but quality
failed. Twelve primary cases completed (eleven empty, one march-only); a rejected
typed response during forest-hunt stopped the remaining 50 cases and all 13 safety
repeats. The export cannot recover rejected scores or the precise invalid response.
A failed 0/63 certification is not 63 independently completed judgments.

Implemented `music-jev-decisions/v3`: direct, complete tag questions; one applicability
Noul per multi-label tag; full period Choice plus absolute applicability; independent
observation support/contradiction Nouls. This removes competing citation probabilities
and the redundant abstract sufficiency question. Numerical cards retain raw values
and add descriptive physical bands; they do not generate moods. Same-observation
ambiguity no longer aborts a run. Specific strict response diagnostics and explicit
not-run case reporting use existing fields and storage. No legacy v2 path remains.

See [the decision record and official research](ADR-026-native-jev-evidence-judgments.md)
for rationale, provisional gates, cost limits and adoption steps. Quality thresholds,
full vocabulary coverage, safety repeats, consent, durable attempts and review remain
in force. Current local audio context and accepted tags are reusable. Live v3 quality
and independent listening remain required; overall progress stays approximately **80%**.

Local validation: all 567 Rust workspace tests pass without skips, including real
FFmpeg and the pinned optional voice-model fixtures. All 374 frontend tests and the
production build pass. Workspace check, strict workspace/fuzz Clippy, formatting,
architecture, doc tests, generated contracts and local documentation links pass.
The native regressions cover independent citations, contradictions, abstention,
period selection, complete 200-tag vocabularies, shared-state batching, conservative
reservations, precise response rejection and durable partial-run recovery. No paid
provider call, push, deployment or claim of improved live music accuracy is included.

## Jev model-test repair — 27 September 2026

The owner's diagnostic export records `conformance_mismatch`, which the server
produces only after successfully parsing a response from the pinned Jev model.
It has no retained answer details, so which judgment failed cannot be recovered.
Its previous quality job belongs to Astra; no Jev quality result is present.

The old test asked Jev to compare random identifier strings, an unsuitable mechanical
probe for its documented semantic strengths. Conformance now asks direct questions
about an explicit synthetic solo-singing description. The nonce remains in question
IDs for local response correlation; model-side random-string comparison is removed.
Positive/negative Noul and Choice gates stay at >=0.90, <=0.10 and >=0.90 respectively.
Specific failures persist in the existing role error field and remain visible after
reload. No new database, service, settings or automatic provider retry is added.

Live Jev conformance, the full unchanged quality suite and independent listening
remain required. Deploy the fix, refresh AI setup and rerun **Test and make available**
with the existing pinned model and verified connection, then run its quality check.
Current audio context and accepted tags do not need a rebuild for this repair.
The overall estimate remains approximately **80%** until live/listening acceptance.

Local validation: 564 Rust workspace tests pass with no skips, including real FFmpeg
and the pinned optional voice model; all 366 frontend tests and the production build
pass. Focused regressions cover identifier isolation, each strict threshold, incorrect
Choice winners, replay/model/schema rejection, persisted errors and the reloaded setup
UI without automatic paid calls. Workspace check, strict workspace/fuzz Clippy,
formatting, architecture, doc tests, generated contracts, frontend lint/typecheck and
local documentation links pass. No paid Jev call or live quality result is claimed.

## Jev group-aware decisions — 27 September 2026

Implemented the owner's Choice/Noul refinement in `music-jev-decisions/v2`.
Period uses one complete Choice with an explicit no-supported-period option;
mood, scene, setting and custom groups retain independent Nouls. A period winner
still needs separate support/sufficiency judgments and selected evidence, sharing
one follow-up request. No new settings, service or storage were added.

Choice probabilities and Noul scores are not comparable: at most one of the eight
candidate slots is reserved for period, with the rest available to multiple tags.
Budgets reserve only one possible period follow-up, and oversized full period
questions fail before cost. The current contract replaces v1 with no compatibility
path; provider/quality freshness changes, while current context and accepted tags
remain usable. The operator guide now includes account/key setup and exact UI steps.
Independent listening and live Jev acceptance remain open; the overall estimate
remains approximately 80%.

Validation: all 562 Rust workspace tests pass, including the real FFmpeg and pinned
voice-model checks. Twelve focused Jev tests cover mixed question types, categorical
abstention, independently grounded period winners, full vocabulary and budget bounds,
and durable mixed period/multi-label execution. Workspace check, strict workspace/fuzz
Clippy, formatting, architecture, doc-test, generated-contract and local-link checks
pass. Frontend and wire shapes are unchanged; no paid Jev call or listening result is
claimed.

## Prior Jev implementation batch — 26 September 2026

The owner requested a native Jev alternative after a rebuilt-context run proposed
only calm. The supplied export contains 50 outcomes: 12 calm and 38 abstentions,
from three successful Astra responses. It has no full provider-visible input snapshots,
so it cannot establish whether the limiting factor was evidence or interpretation.
The text prompt uses calm as its positive example, a plausible anchoring influence
that is not a proven cause. That current engine remains selectable for comparison.

**Implemented:** native System One HTTP, pinned model selection despite alias-only
discovery, dedicated Noul/Choice conformance, complete vocabulary partitioning,
separate support/sufficiency gates, model-selected observation citations, application
explanations and conservative tentative proposals. The existing quality suites,
review/storage contract, profile freshness and durable usage ledger are reused.
Preview includes worst-case follow-ups; unknown-cost failures are never replayed.
No SDK, service, database, generated-audio upload or automatic provider fallback was added.

**Still required:** operator configuration, live Jev conformance/quality checks and
independent listening against the same recordings. No paid provider run, deployment
or improvement in actual mood accuracy is claimed. Use the
[setup instructions](../ASSISTANT.md#trying-jev-for-mood-tagging). The overall estimate
remains **approximately 80%**, because listening usefulness and production acceptance
are still open. Jev implementation is now in the requested scope; adoption remains
conditional on results.

Local validation: all 559 Rust tests pass with FFmpeg and the pinned optional voice
model configured; workspace check, strict workspace/fuzz Clippy, formatting,
architecture, doc-test and generated-contract gates pass. All 362 frontend tests,
lint, typecheck and the production build pass. Native fixtures exercise full question
and evidence processing through durable jobs, not live Jev accuracy. Planning the
63 synthetic cases fell from about 22 seconds to 5 seconds in the same debug test
after removing repeated growing-request validation; this is not provider latency.

## Prior implementation status — 25 September 2026

- **Implemented:** selected-library inventory and saved-vocabulary exports initialize
  the grouped JSONL pilot before model calls. Frozen recording groups, four-state
  labels, development/confirmation scoring, per-tag counts, known-positive recall
  and paired group-bootstrap comparisons remain. Empty/failed run exports preserve
  missing outcomes; model-success-based initialization is removed. The current v2
  pilot freezes recording duration and requires blind, complete-recording judgments
  for independent scores. Assisted/excerpt comparisons require explicit diagnostic
  mode and retain the entire cohort. Freezing works before listening. The owner has
  no labeled dataset yet; listening remains open. A read-only status command now
  reports the selected split's blockers, listening time and four-state label counts
  before any run export exists, using the same gates as scoring and comparison.
- **Implemented:** context v3 with gain-invariant relative dynamics, explicit coverage,
  no whole-track context confidence, `voice_score`, and coarse local tempo withheld from
  the model projection. Existing bounded execution and source-audio decoding are reused.
- **Implemented:** bounded original Last.fm observations, current-policy MusicBrainz/
  Last.fm projection in tagger input v24 (`song-evidence/v1`), source-aware result identity,
  transactional save/review guards, updated disclosure v15 and runtime fingerprints.
- **Implemented:** forward schema-15/16 reset of generated analysis and proposal reviews,
  old-job supersession and audit preservation, current-only context parsing, updated
  existing review/inspector UI. Accepted/manual tags and authored playlists survive.
  Forced retries now preserve current completed results from compatible predecessor
  jobs; restart/retry, missing-file recovery and stale-source regressions use SQLite.
  New jobs retry failed optional voice without repeating current factual extraction;
  same-job restart retains its saved failures without repeated attempts.
  Analyzer task panics fail the job while preserving fixed worker capacity for an
  explicit retry; completed checkpoints survive the failure.
- **Implemented:** read-only factual and voice acceptance probes using the real extractors,
  with bounded repeats, explicit cancellation outcomes and shared input/memory helpers.
  Voice reports readiness, per-track work and each worker start/join separately; scores
  appear only for completed classification. Both current v3 reports optionally capture
  an explicit Linux cgroup v2 scope, with local limits, memory/CPU and OOM/throttling
  counters. No model, dependency or storage layer was added. Live container totals and
  concurrent playback need separate measurement.
- **Implemented:** bounded final loudness-report capture survives verbose embedded notes;
  factual decode/loudness codec and filter pools are limited. The acceptance probe exposes
  numeric measurements. Factual decoding has a 30-minute budget; factual and voice
  cancellation/deadlines remain active through decoder exit, with typed timeout failures.
  The direct ebur128 substitution failed end-of-file/short-signal
  checks and was rejected; no faster loudness algorithm has been adopted.
- **Implemented:** shared MusiCNN frame preprocessing with independently generated,
  checksum-pinned numerical fixtures and reusable FFT scratch. All 1,152 synthetic
  frame features pass the fixed tolerance; the frame transform definition is unchanged.
- **Implemented:** complete ending windows and constant-storage voice summaries, strict
  invalid-value/cancellation handling, normalized stereo input and bounded FFmpeg pools.
  Analytic stereo spectra and out-of-band suppression pass at 44.1/48 kHz; a degraded
  filter fails the new gate despite correct counts. Full resampler parity remains open.
  The exact pinned graph and real FFmpeg/worker tests now run; decoder/window identities
  make older generated contexts stale without changing accepted/manual tags.
- **Implemented:** each voice worker verifies the exact owned model bytes it parses,
  with bounded input and replacement/deletion/recovery regressions. The artifact/v2
  identity retires contexts produced under the previous startup-only verification.
- **Native probe:** five synthetic patches, 66 selected real patches and all 5,503
  patches across the 22 approved recordings pass fixed numerical gates. The offline
  reference now streams centered frames, preserves silence/endings, partitions time
  for summaries and handles cooperative cancellation. A 15-minute synthetic limit
  case also passes native comparison. The original TensorFlow encoder and matched
  heads now agree on 71 synthetic/real patches, including a batch-position check.
  Both reference exporters bind scores to pinned label metadata. Production worker
  lifecycle/resources and independent listening usefulness remain open; no
  application model runtime or weights are bundled.
- **Implemented:** tagger output v5 / analyzer v8 with per-tag support, reasons,
  validated supporting/conflicting observation IDs and explicit abstention. Current-only
  save/review contracts, strict fixtures, disclosure and existing review UI are updated.
- **Implemented:** retired metadata/audio heuristic jobs, routes, schemas and UI; removed
  their saved axes and readers. Automatic playlists use accepted/manual tags. Playlist
  ranking retains metadata search but no longer presents keyword guesses as analysis tags.
- **Still required before production acceptance:** independent owner judgments,
  real-library listening comparison, resource/concurrent-playback checks, and an
  operator-started production rebuild. No paid calls or production changes were made.
- **Conditional:** learned audio integration follows its parity/usefulness gates. Jev,
  training, extra encoders/datasets and a new annotation UI are not release dependencies.

### Estimated progress and remaining acceptance

Estimate for the agreed lean scope, recorded 25 September 2026. These are engineering
judgments, not test coverage or mood-accuracy percentages. Count required delivery and
acceptance work; do not count an optional model as delivered or assume every research
option must be implemented. Revisit the estimate if listening or production tests expose
required changes.

| Workstream | Weight | Estimated completion | Evidence or remaining work |
|---|---|---|---|
| Core application implementation and clean cutover | 60% | 100% | Current factual/voice/evidence/review contracts, reset, retry/recovery and legacy removal implemented. |
| Automated and local technical qualification | 20% | 90% | Numerical, synthetic, approved-original and recovery checks exist; full resampler/multichannel qualification remains incomplete. |
| Independent listening and session usefulness | 10% | 0% validated | Pilot tooling exists, but no owner-labeled cohort or independent benefit result is available. |
| Production resource/playback acceptance and full rebuild | 10% | 0% validated | Tooling is ready; target-container observations and an accepted rebuild are not recorded. |

Weighted completion is **78%, rounded to approximately 80% overall**. Core code being
implemented does not make the delivery production-accepted. EffNet/Jev adoption and the
other conditional alternatives were outside this estimate. Jev entered implementation
scope on 26 September; live qualification remains open, as recorded above.

### Local validation and release boundary

The listening-readiness batch passes all 35 offline pilot tests, including real CLI
checks with no model-run file, unchanged-input checks, confirmation isolation and
shared readiness/scoring/comparison gates. The Rust application, frontend, providers,
model runtimes and wire contracts are unchanged; their suites were not rerun.

The preceding spectral-validation batch passed all 544 Rust tests on Windows GNU
with the real pinned voice model and FFmpeg configured, plus workspace check,
strict workspace/fuzz Clippy, formatting, architecture, doc tests and generated
contracts. Six synthetic decoder cases verify stereo spectra and out-of-band
rejection; a degraded-filter negative control failed despite correct counts.
Existing resource, decoder, panic and eight SQLite-backed recovery cases passed.
No live Linux/cgroup or production-playback validation ran in either batch.

Earlier batches passed the headless release build, all 361 frontend tests, frontend
lint/typecheck/production build, 29 grouped mood-pilot tests and 26 reference-tool tests.
Original TensorFlow/ONNX comparisons passed on 71 patches; the metadata-bearing formats
reproduced all 66 selected patches and a complete 226-patch track with its summary.
Frontend, model-reference tools and packaging are unchanged by this batch;
their separate gates were not rerun. The pilot suite increased from 29 to 35 tests.
The migration test starts with the old schema, verifies the backup/reset, parses a
preserved automatic rule with its new tag source, and confirms fresh results survive
reopening. Browser guards reject the retired result shape.

No Docker host was available for the production-image smoke test. Visual inspection
could not initialize because the Codex browser helper failed with a local ACL error;
component interaction tests passed. Owner listening, production resource/concurrent-
playback checks and the live rebuild remain separate acceptance work. No private
library judgments were invented, and no provider calls, push or deployment occurred.

### Batch progress and tool inventory

**Latest batch:** added a read-only listening-readiness command to the existing
pilot CLI. Before running a candidate, the owner can inspect blocked track IDs,
ready/reviewed counts, declared listening time and unknown/uncertain core labels.
It verifies the frozen membership/vocabulary and uses the same listening gates as
score/compare. Confirmation stays explicit; no predictions or new annotation UI
are needed. All 35 pilot tests pass, including actual CLI/read-only checks.

Diagnosed the earlier reference-resampler exception: the pinned Essentia.js build
has a JavaScript wrapper but no registered `Resample` algorithm. Its caught exception
says `Identifier 'Resample' not found in registry...`; full upstream resampler
parity is therefore still unverified. This is a development-reference limitation,
not evidence of a production decoder defect. No runtime dependency or filter changed.
See the [spectral acceptance evidence](AUDIO_ANALYSIS_ACCEPTANCE.md#decoder-spectral-acceptance-25-september-2026).

**Fully implemented in the application:** factual whole-track context and optional
voice analysis; attributed catalog evidence; structured per-tag model proposals and
abstention; stale-result/review guards; current-only reset/resume; removal of the
old keyword/audio mood heuristics; accepted-tag playlist behavior. Read-only probes,
pilot inventory/run exports, pre-call listening readiness and offline comparison tools
are also implemented.

**Experimental only:** EffNet and its matching heads pass numerical qualification,
including whole-track native comparison and original TensorFlow/ONNX pairing. They
are not integrated into application workers, evidence, storage or review. Jev now has
a native application adapter; other conditional alternatives have no adoption commitment.

**Still open:** owner labels and listening/session-use comparison, application/container
resources with concurrent playback, and an operator-started full-library rebuild.
EffNet still needs production worker integration/lifecycle qualification if adopted.
No model has earned a mood-accuracy claim. No provider call,
audio upload, authored-tag change, push or deployment was made.

For every subsequent batch, update the delivered work, checks, remaining gate and
this inventory. Importance reflects this product's needs, not model popularity.
Conditional items are options requiring an observed failure and a measured benefit;
they are not all scheduled for implementation. The original research is a dated
options survey; this plan determines the narrower implementation scope.

| Tool or approach | Old use | Current use | Planned decision | Importance and value; reason |
|---|---|---|---|---|
| Metadata-keyword mood analyzer | Title/genre/album guesses | Removed; Jev also excludes retrieval context cues from tag meaning | Keep removed; retain request retrieval separately | Remove: lexical associations were not independently grounded mood evidence. Ordinary metadata search remains useful. |
| Audio energy/brightness/tension mood rules | Heuristic generated tags and saved axes | Removed | Keep removed | Remove: loudness and spectral measurements do not establish emotional meaning. |
| FFmpeg / ffprobe | Decode and technical inspection | Bounded pools/downmix/deadlines plus analytic spectral and alias-suppression checks | Keep | Core: preserve original-audio content and detect spectral damage that correct duration/counts can miss. |
| FFmpeg loudnorm / ebur128 | Loudnorm input measurements | Loudnorm retained; direct scanner comparison failed | Keep loudnorm until independently validated replacement | High correctness priority: a faster scanner missed an ending peak and disagreed on short-signal range. No speedup claim. |
| music-context-probe / music-voice-probe | No factual acceptance CLI; basic single-pass voice probe | Current v3 reports; repeats/cancellation/timeouts, coverage, worker lifecycle and optional scoped cgroup snapshots | Keep for rebuild acceptance | High: observe decoder-inclusive resource scope without inventing totals from process RSS; live production/playback checks remain required. |
| RustFFT factual context | Older DSP and global confidence | Context v3, relative dynamics and coverage | Keep and measure | Core: local changes, endings and dynamics can help reject unsuitable session music; confidence is not inferred from duration. |
| Coarse tempo estimator | 20 Hz integer-lag estimate | Local inspection only; omitted from tagger evidence | 100 Hz onset/interpolation only if rhythm errors matter | Conditional: improve pulse accuracy when it changes actual selection; no rhythm project by default. |
| MusiCNN voice classifier + tract-tensorflow | Optional local voice estimate | Tract 0.23.8; ending coverage, verified model bytes, bounded summaries and selective failed-stage retry | Keep optional | High session value: audible vocals need correct input and dependable model attribution; window scores remain uncalibrated. |
| Essentia.js / ONNX Runtime Web / TensorFlow references | Synthetic frame/patch checks | Whole-track checks, original-export comparison and checksum-bound label order | Keep development-only; TensorFlow stays outside the repository/image | High: establishes numerical pairing and score meanings without adding an application runtime. |
| AcoustID / Chromaprint | Recording identification | Existing conservative identity matching | Keep | Core for source matching: prevents attaching facts to the wrong recording; does not verify mood or equivalent editions. |
| MusicBrainz | Recording/catalog enrichment | Current-policy recording genres, composer/date claims in shared evidence | Keep | High: attributable recording context without pretending catalog genres are listening judgments. |
| Last.fm | Community tags, exact vocabulary mapping | Bounded original tags/counts with weak-source attribution | Keep bounded | Supporting: useful descriptors and vocabulary, but community counts are neither ground truth nor independent votes. |
| Structured text model tagger | Whole-track confidence and tag list | Per-tag support, evidence/conflict references, abstention and v25 provenance distinction | Keep with review | Core optional interpretation: combines permitted evidence with the owner's vocabulary; never writes accepted tags itself. |
| SQLite / durable jobs / review guards | Forced retries repeated facts; normal runs retained failed voice indefinitely | Current-only reset/freshness, compatible retry-chain reuse and voice-only failure recovery | Keep | Core: retry failed work without repeating current facts or looping on same-job failures; preserve authored tags and reject stale reviews. |
| Bounded Rust analysis executor | A task panic permanently removed a worker | Typed panic failure; same fixed worker remains usable | Keep | High reliability: an unexpected extraction failure cannot disable later analysis; no automatic retries or larger pool. |
| JSONL listening pilot + grouped bootstrap | Small/result-derived sample; assisted/excerpt scores mixed with independent listening | Explicit inventory, frozen duration/groups, read-only readiness, independent whole-recording scoring, marked diagnostics and paired comparison | Use readiness to finish independent judgments; evaluate development, then confirmation | Essential: prevents selection/listening-scope bias and exposes unfinished listening before model calls; no dataset application. |
| Discogs-EffNet + matching MTG-Jamendo mood/theme and instrument heads | Not used | Whole-track native checks pass; original TensorFlow pairing passes on 71 patches | Qualify production lifecycle/resources and listening usefulness before adoption | Conditional high value: verified graph pairing and label identity support a pilot; they do not certify mood quality. |
| tract-onnx / ort | Neither in production | Tract 0.23.7 passes isolated whole-track streaming and summary comparison | Prefer Tract if fully qualified; native ORT only if required | Conditional infrastructure: retain one production runtime; native worker integration still requires admission gates. |
| TypeSafe Jev | v5 reached 15/66; v6 45/66; v7 55/66 | V9 restores v7 questions; criteria removal rejected with zero gains, four losses and one forbidden candidate | Keep explicit criteria; audit literal metadata misses, qualify acoustics, then independent listening and full validation | Requested high priority: no fresh certification; preserve quality and safety gates. |
| Bounded Jev developer comparison/quality runner | Manual logs, then limited question comparisons | Criteria-removal comparison rejected; source/amount experiment prepared with shared journal guards, skipped no-ops and repeat controls | Run only the separately approved data plan, inspect paired tradeoffs, then fully validate promising changes | High debugging value: determine useful evidence without changing questions or adding a production service/dependency. |
| LAION larger_clap_music | Not used | Research option | Compare only for a remaining semantic/retrieval gap | Deferred: flexible text/audio matching; similarity is not probability and runtime cost must be justified. |
| MSD-MusiCNN + DEAM head | Not used | Research option | Probe only if affect dimensions remain weak | Deferred: valence/arousal evidence; requires its own matching encoder, not the existing voice output. |
| Beat This! | Not used | Research option | After a demonstrated failure of simpler rhythm repair | Deferred: beat/downbeat detail only when useful to selection; adds native integration and resource work. |
| L2 logistic heads (linfa-logistic), source combiner and calibration | Not used | No trained local model | Only with sufficient independent grouped labels and a quality/offline/cost need | Deferred: a small local alternative may help; do not train on generated tags or average unrelated scores. |
| MTG-Jamendo / DEAM / OpenMIC datasets | Not imported | Reference datasets only | Narrow import for a specific label/domain question | Conditional: preserve partial labels, splits, licensing and version scope; not substitutes for owner judgments. |
| MusicBrainz work relations / Discogs / Wikidata | No added analysis adapters | Research options beyond current projection | Add only a missing fact with a useful consumer | Conditional: extend attribution without collecting unused catalog fields. |
| Content hashes / NPY artifacts (npyz) / embedding cache | File facts used for freshness | No new tensor store; pilot accepts private content references | Add only for measured invalidation, reuse or training needs | Deferred: cache maintenance and storage need a demonstrated saving or consumer. |
| MuQ / MuQ-MuLan / Cyanite | Not used | Research challengers | Only if smaller choices fail an important use case | Low current priority: larger local resources or explicit external audio upload and recurring cost. |
| All-In-One structure analysis | Not used | Research reference | Not in delivery scope | Low: pop-section semantics and source-separation cost have no demonstrated tabletop benefit. |
| Annotation UI / active learning / vector database / large-model fine-tuning | Not used for this rework | No new platform | Excluded by default; reconsider only a concrete bottleneck | Avoid bloat: pilot files, existing review and SQLite cover current needs. |

### Native probe evidence

An isolated release-build Rust probe on the Windows GNU host loaded all three artifacts.
The ONNX encoder's real output names are `activations` and `embeddings`, and both heads
use `activations`; the catalog JSON names describe a different exported graph interface.
Use `with_ignore_value_info(true)` to let Tract infer intermediate shapes after binding
batch size one; otherwise symbolic `batch_size` value-info conflicts with the concrete input.
No graph operations or weights were rewritten. Encoder input is `[1,128,96]`, embedding
`[1,1280]`, head inputs `[1,1280]`, outputs `[1,56]` and `[1,40]` respectively.

| Artifact | Verified SHA-256 |
|---|---|
| `discogs-effnet-bsdynamic-1.onnx` | `a280825b334797cf677939db8cd5762c0392aedd0ca6415dbc1cd083f045e43c` |
| `mtg_jamendo_moodtheme-discogs-effnet-1.onnx` | `7d6270acaa5f4bba4b115a0d6849aca05ed6bd153dcb6d9da4f6ab9f99ef10ff` |
| `mtg_jamendo_instrument-discogs-effnet-1.onnx` | `9ae2d9e763d66bd8eed654d1ac3aa171e6539cb8a0e11f3dcd53df1428980802` |

The initial zero-input smoke test was supplemented by five synthetic 128-frame
patches. Essentia.js 0.1.3 supplies reference mel features and ONNX Runtime Web 1.30.0
(single-thread CPU WASM) runs the same published graphs. Both graph-only comparison
and the shared Rust frontend plus Tract meet the fixed feature/embedding/head gates.
All values are finite with the expected dimensions; worst mel error is 0.0000290871
and worst head error is below 0.00000090. See the
[reference ledger](../crates/music-analysis/tests/fixtures/README.md#isolated-effnet-comparison-24-september-2026)
for the cases and limits. This is controlled numerical evidence, not original
TensorFlow-export equivalence, real-music coverage, production resource acceptance
or mood-quality evidence. The artifacts and temporary native probe remain ignored
research output, not application dependencies. Upstream links and licensing remain in stage 2.

The 25 September real-audio extension passed 66 selected patches from the two approved
albums using the same gates. It replaces the silence-dropping convenience frame helper
with explicit positions in the shared decoded PCM. The
[reference ledger](../crates/music-analysis/tests/fixtures/README.md#real-audio-patch-reference-25-september-2026)
records the patch exporter and 811,008 feature comparisons. The subsequent
[whole-track experiment](../crates/music-analysis/tests/fixtures/README.md#whole-track-stream-reference-25-september-2026)
compares all 5,503 real-audio patches and time-weighted summaries, plus a 907-patch
synthetic limit case. The [original-export comparison](../crates/music-analysis/tests/fixtures/README.md#original-export-pairing-and-label-identity-25-september-2026)
now qualifies the named TensorFlow/ONNX pairing on 71 patches. Independent decoding,
production lifecycle/resources and listening acceptance remain open.

## Product purpose and admission rule

Music is a single-operator, self-hosted music player and tabletop-session orchestrator.
Song evidence should reduce the work of finding suitable music and preparing normal
playlists/sessions. Everyday library use and dependable playback remain primary.
Analysis is optional preparation; playback must never depend on a model or provider.
Follow the existing [Assistant and Authoring workflow](assistant-ux-philosophy.md).

Every addition must name an observed problem, its consumer, a measurable benefit,
and the simplest viable solution. Include setup, compute/storage, provider cost,
maintenance, failure recovery and removal. Complexity is justified when a
simpler choice demonstrably fails an important use case; an unused field is not progress.

Example outcomes to validate: a quiet exploration bed without disruptive vocals or
climaxes, sustained tension, energetic combat, or a desired listening mood. Evaluate
perceived sound and session suitability separately within existing product workflows.

## Clean cutover: recompute all generated analysis

Replace the analysis contract and rebuild every indexed recording from its source audio.
Remove superseded analyzers, schemas, parsers, aliases, pilot formats, compatibility
branches and old-engine fallback. Do not translate or reuse old generated results.
Only results produced by the new pipeline can satisfy analysis or review freshness.

Preserve source files, embedded metadata, track identities, attributable catalog facts,
accepted/manual tags, independently collected judgments and authored playlists/campaigns.
Refresh external observations under current source policy. Clear derived contexts,
features, predictions and proposal-bound review state; retain paid-attempt accounting
only as non-executable audit history. A failed or unfinished rebuild remains visibly
unavailable instead of displaying old results. Ordinary playback remains usable.

## Recommended first delivery

Reuse catalog and job infrastructure and update the tagger in place; keep Mood Library
review and the playlist workflow. Trial one additional audio model family and retain
only the evidence that improves decisions. Jev is an optional comparison on the same
evidence, reflecting the owner's ongoing investigation; it is not a release dependency.

| Component | First delivery | Expand only when |
|---|---|---|
| Evaluation | Small owner-judged, grouped pilot and untouched confirmation cohort | Results are inconclusive, more tags matter, or training needs more examples |
| Audio | Probe Discogs-EffNet with matching mood/theme and instrument heads; adopt only useful heads | A measured failure warrants another model |
| Native inference | Probe `tract-onnx` matching existing Tract; `ort` only if necessary | Compatibility and quality justify native packaging cost |
| Storage | Replace the generated-data contract in SQLite; add one bounded summary per track/analyzer if learned audio is adopted | An actual consumer needs retained tensors, history, or cross-file reuse |
| Interpretation | One new tagger contract with improved evidence; optional Jev comparison | Local training offers a measured quality, offline-use, or cost advantage |
| Review | Existing Mood Library and Authoring transactions | Repeated operator friction justifies a small UI extension |

Keep measured sound, learned predictions, catalog claims, listener judgments and
session suitability distinct. Verified recording identity does not verify mood.
Use all relevant, permitted evidence already available; acquire more sources only
when a missing field would change a useful decision. Source count is not a quality target.

Follow stages 1-7, skip optional stage 8 unless needed, then integrate and release
successful parts through stages 9-10. If stage 2 rejects learned audio, skip its storage
and extraction; catalog/DSP improvements can proceed. Stop expanding when the owner's
needs are met. The conditional backlog is not a commitment.

## 1. Define useful outcomes and establish a small honest baseline

**Owners:** [vocabulary](../crates/music-application/src/assistant/vocabulary.rs),
[pilot tooling](../tools/mood-pilot.mts), existing Mood Library review.

- Record a small fixed set of real listening/session requests and current failures.
  Measure useful candidates found, auditioning time, rejected suggestions and review
  effort using the existing planner/review flow. Keep track suitability separate from
  tag accuracy; more tags need not make selection easier.
- Preserve tag IDs, the four groups, and the eight-suggestion limit. Define a small
  core of common tags with positive and confusable examples. Judge every core tag
  per selected recording; do not attempt to certify all 138 default labels at once.
- Replace the 30-track pilot format and fixtures with one grouped JSONL judgment/
  manifest format. Remove the old mode and parser. Start with roughly 60-100 representative
  recordings if available, including actual failures and a random library sample.
  This is a pilot size, not a statistical claim or a prerequisite for correcting defects.
- Store stable recording/file references, vocabulary revision, grouping, split seed,
  annotator, complete-recording duration, listened intervals and blind/assisted status.
  Keep related versions, duplicates and excerpts in one partition. Separate composers/albums where feasible;
  report residual overlap. Use grouped development and untouched confirmation cohorts
  initially; a training/calibration split becomes necessary only for fitted models.
- Label `positive | negative | uncertain | unjudged`; omission is unjudged. Score only
  judged positives/negatives and report judgment coverage. Do not turn all unselected
  tags into negatives. Record whole-track versus excerpt scope explicitly. Independent
  scoring requires blind judgments covering the complete frozen duration without gaps
  for the whole selected split; assisted/excerpt scores require marked diagnostic mode.
  Never improve apparent quality by silently dropping those tracks from the cohort.
- Collect perceived-mood judgments without showing predictions where practicable;
  collect session-use judgments separately. The owner's preferences are the primary
  product target. A second listener can investigate ambiguity; broad multi-listener
  annotation is needed only for claims beyond that owner. Preserve disagreement and
  known blinding limitations. Keep private audio and judgments outside Git.

**Gate:** capture baseline A once before replacement as a private research report;
the new runtime and pilot tools do not load old analysis formats. Freeze its configuration;
lock confirmation groups before tuning. Verify grouping, partial-label scoring and
missing-versus-abstained reporting with fixtures. Report per-tag counts and recording-group
bootstrap intervals; small samples cannot certify rare tags. Reuse existing consent
for baseline collection.
A new general dataset CLI or annotation application is unnecessary for this pilot.

## 2. Resolve native model feasibility before building around it

**Owners:** [analysis crate](../crates/music-analysis/src/lib.rs),
[voice implementation](../crates/music-analysis/src/voice.rs), optional probe binary.

First compare reference head scores on a few development failures through an experimental
evidence projection. Use updated disclosure/consent for external calls. Require a plausible
decision benefit before production storage/integration; otherwise stop this model branch.
Then probe one EffNet stack, with no parallel production runtimes or encoders.
Use a small licensed reference corpus containing silence, impulses/tones, short clips,
stereo, ordinary music, and a long changing recording. Produce reference tensors and
outputs with pinned upstream tooling in an isolated research environment. Keep upstream
Python tooling outside this Rust-only repository and release image; retain manifests
and permitted numerical fixtures, not private audio or model weights.

| Probe | Exact implementation candidate | Decision |
|---|---|---|
| Encoder | `discogs-effnet-bsdynamic-1.onnx`: published input `[n,128,96]`, embedding output `PartitionedCall:1`, 1,280 values; 16 kHz audio preprocessing | Preferred first baseline. Inspect actual ONNX tensors: its JSON even links to a `.pb` filename despite declaring ONNX. [Metadata](https://essentia.upf.edu/models/feature-extractors/discogs-effnet/discogs-effnet-bsdynamic-1.json), [actual artifacts](https://essentia.upf.edu/models/feature-extractors/discogs-effnet/) |
| Heads | `mtg_jamendo_moodtheme-discogs-effnet-1.onnx` and `mtg_jamendo_instrument-discogs-effnet-1.onnx`; 56 and 40 scores | Validate the ONNX encoder against the documented TensorFlow encoder/head pairing. Never attach these heads to voice-model activations. [Mood metadata](https://essentia.upf.edu/models/classification-heads/mtg_jamendo_moodtheme/mtg_jamendo_moodtheme-discogs-effnet-1.json), [instrument metadata](https://essentia.upf.edu/models/classification-heads/mtg_jamendo_instrument/mtg_jamendo_instrument-discogs-effnet-1.json) |
| Runtime | Probe `tract-onnx = 0.23.7` with concrete batch size 1, matching current `tract-tensorflow` | Gate on supported operators, numerical parity and resources; matching versions alone proves nothing. If unsupported, compare a pinned CPU ONNX Runtime through `ort`, including native-library packaging. [Pinned Tract API](https://docs.rs/tract-onnx/0.23.7/tract_onnx/), [ort](https://github.com/pykeio/ort) |

Create a model manifest with artifact SHA-256, source revision, license, tensor names,
preprocessing version, label order, runtime and permitted use. Essentia's published
MTG weights use CC BY-NC-SA/proprietary licensing; repository code licensing is a
separate matter. Keep installation explicit, checksum-verified and optional.
[Model licensing](https://essentia.upf.edu/models.html).

**Gate:** load exact graphs, compare preprocessing and outputs to the reference,
measure cold/warm latency, peak RSS, cancellation and memory release. Proposed
starting parity criteria: normalized embedding cosine >=0.999 and head absolute
error <=1e-3 on nonsilent fixtures, with separate near-zero handling. Investigate
systematic differences; do not loosen tolerances to hide a preprocessing mismatch.
Run within the documented [three-CPU/4 GB resource contract](RUST_REWRITE_ARCHITECTURE.md#non-functional-requirements)
before production adoption. Verify the exact bytes imported by each worker, not only
a file hash observed at service startup. The voice path now enforces this with a
bounded owned snapshot; future model loaders must preserve that guarantee.
A failed probe records rejection or justifies one alternative runtime; it does not block useful
catalog/DSP corrections. Benchmark both heads, but ship only heads with a useful consumer.

## 3. Define the new storage contract and one-way reset

**Owners:** application `assistant` evidence types,
[storage](../crates/music-storage/src/analysis.rs),
[migrations](../crates/music-storage/src/migration.rs) and
[schema checks](../crates/music-storage/src/schema.rs).
Keep the eight-crate structure and current dependency direction.

| Record | New contract and cutover treatment |
|---|---|
| `track_contexts` | Clear old rows; recompute factual DSP/voice evidence from source audio |
| Proposed `track_audio_features` | One bounded record per `(track_id, analyzer_id)`: pipeline/source/model/preprocessing signatures, coverage/status, aggregate scores and selected intervals; no old-cache import |
| Catalog observations/result JSON | Preserve attributable source facts under current policy; refresh unsupported/expired cached payloads instead of adding readers for old formats |
| `track_analyses` | Replace the result shape with per-tag support/abstention and evidence references; clear old interpretations and remove obsolete columns |
| `track_user_tags` | Preserve accepted/operator-owned tags independently of generated analysis |
| `track_analysis_tag_reviews` and analysis failures | Clear state tied to superseded results; new proposals start a new review lifecycle |
| Private pilot files | One current manifest/judgment format; static pre-change research reports are not application inputs |

Use measured, catalog-observed and model-predicted input types. Missing, unavailable,
failed, partial and complete are explicit; missing is not zero. Per-tag decisions carry
support/abstention, bounded evidence references, contradictions and temporal scope.
Raw scores differ from optional demonstrated calibrated probabilities. Remove legacy
track-level confidence from analysis storage, DTOs and UI; do not carry a compatibility
summary or placeholder. Validate finite values, dimensions, references and payload limits.

Use forward SQLx migrations that resets derived records and removes obsolete
schema objects. Keep applied migration history intact; register the cutover migrations
with the matching writers/readers and old-path deletion in stage 9. It must preserve
source/authored data without translating old analysis payloads. Supersede old queued
analysis work before job recovery; completed/uncertain paid attempts remain audit-only
and are never resumed or reinterpreted as fresh results. Expire old analysis-role
certification/consent fingerprints and old review links under the new contract.

Use a proposed 64 KiB maximum learned-feature payload per track. Store summaries in
SQLite; retain full tensors only for selected private research fixtures. A separate
ledger/artifact store needs a concrete query or retention requirement.

**Gate:** fresh-database and upgrade/reset fixtures reach the same current schema;
reset preserves accepted tags and authored resources, rejects old result/job/review
identities, and recovers from interruption without exposing partial state. Repeated
startup preserves completed new-pipeline work; the reset migration runs once. Doctor and
backup/restore tests use the new contract. Restoring an older backup runs the same
one-way reset before analysis is available; there is no legacy-reader mode.

## 4. Connect existing sources and preserve correct invalidation

**Owners:** evidence projection;
[catalog workflow](../crates/music-application/src/cleanup_enrichment/workflow.rs),
[typed catalog port](../crates/music-application/src/cleanup_enrichment/catalog.rs),
[catalog invalidation](../crates/music-storage/src/catalog_evidence.rs).

- Include the new pipeline revision plus source/model/preprocessing/runtime identity
  in every generated result and job. Force fresh extraction for every track at cutover,
  regardless of matching old file signatures; subsequent reuse accepts only new-contract
  results. Bind decisions to allowed metadata, observation revision, source policy,
  vocabulary, engine and question/prompt revision.
  Recheck these dependencies inside the write transaction. Preserve the existing
  signature's limitations; add content hashing if it proves insufficient for correct
  invalidation. Full decoded-PCM identity and reuse across renamed files are optimizations,
  not a prerequisite for better mood evidence.
- Reuse the existing conservative MusicBrainz/AcoustID matcher and imported evidence.
  Preserve entity and recording/version scope. MBIDs and fingerprints can help group
  evaluation families; they do not prove interchangeable audio across remasters or edits.
- Retain original Last.fm tags/counts before exact-alias filtering in the existing
  observation payload. Mark them as external claims, including unmapped tags, and
  project only relevant, permitted entries under a size bound. An artist genre is not
  recording mood; several copied tags are not independent corroboration. Start with
  sources already configured. Extra work/composer relationships need a demonstrated
  decision benefit before expanding catalog fetching or typed responses.
- Read enabled, current observations through one shared projection. Changing source
  policy or disabling a source expires dependent suggestions, preserving accepted
  manual tags. Exclude generated suggestions and review history from ordinary model
  input. Keep identity details that are unnecessary for inference local.
- Build the pilot from the owner's existing library and reviewed identity groups.
  External dataset importers are optional experiments for a specific question, not
  a new ingestion platform. A public dataset's labels do not become ground truth for
  a local recording without an exact version match and matching annotation scope.

**Gate:** rejection of pre-cutover results, source/model invalidation, ambiguous-match
exclusion, source-disable races, bounded raw-tag handling and duplicate-family grouping. A tag-only file edit
may trigger fresh analysis initially; measure that cost before adding another cache.

## 5. Correct misleading factual inputs and share required preprocessing

**Owners:** [context DSP](../crates/music-analysis/src/context.rs),
[voice streaming](../crates/music-analysis/src/voice.rs),
[shared mel frontend](../crates/music-analysis/src/musicnn.rs).

- The shared MusiCNN frame frontend is implemented behind pinned numerical fixtures.
  Keep it limited to the voice classifier and successful EffNet probe: 16 kHz, 512-sample frames, 256-sample hop, 96 Slaney mel bands
  and log compression. EffNet uses this feature family but **128-frame patches**,
  unlike the voice model's 187. Check centering, downmixing, resampling, silence and
  tails against the pinned reference. [EffNet preprocessing](https://essentia.upf.edu/reference/std_TensorflowPredictEffnetDiscogs.html).
- Voice decoding now matches the pinned MonoMixer's levels for mono/stereo and passes
  native-rate, 44.1/48 kHz count/level, ending, invalid-value and cancellation regressions.
  One final full patch covers the ending without repeating tail frames; summaries use
  constant storage. Decoder/window identities invalidate previous generated contexts.
  Analytic 1/4 kHz stereo spectral and 9/12 kHz stopband checks now pass at 44.1/48 kHz.
  They catch a degraded resampler that retains correct counts. Full upstream resampler
  parity, multichannel behavior and the production resource gate remain separate.
- Stop treating the current loudness-heavy intensity proxy as independent evidence
  of emotional arousal, or duration/activity heuristics as calibrated accuracy.
  Keep absolute loudness for technical uses; trial relative dynamics where it helps
  detect unsuitable climaxes. Preserve the distinction between coverage and accuracy.
- The current 20 Hz/integer-lag tempo estimator is coarse. Mark its uncertainty or
  omit unreliable tempo evidence from mood decisions first. If the pilot shows rhythm
  errors drive bad selections, implement a 100 Hz onset envelope, peak interpolation,
  interval checks and half/double-tempo candidates, including unstable/no-pulse output.
  Validate 113/127 BPM, rubato, tempo changes and no-beat recordings. This rhythm upgrade
  does not block catalog or learned-feature improvements.
- Version any changed factual semantics. Learned mood/instrument outputs remain in
  the separate feature record. Defer chroma/key features until an ablation shows value;
  major/minor mode must not become a mood rule.

**Gate:** relevant gain-change, clipping, silence and short-file fixtures; no voice
inference regression. Any adopted rhythm change needs real annotated music as well
as synthetic metronomes. Repair misleading semantics even if no new model is adopted.

## 6. Extract bounded audio evidence without a feature-storage platform

**Owners:** analysis feature module, application feature port/job,
[server composition](../crates/music-server/src/analysis.rs), existing analysis executor.
For learned audio, depends on the successful parts of stages 2-5.

- Run one bounded full-library rebuild through the existing durable job/executor
  infrastructure, defaulting to one worker. Recompute factual DSP and every enabled
  voice/learned-feature stage from original audio for every indexed track. Checkpoints contain
  only new-pipeline work; interruptions resume that rebuild without importing old results.
  Missing/failed files remain explicit. Cancellation never marks partial work complete.
  Model availability remains independent of boot/playback; release memory after the pass.
- Run the selected EffNet encoder once per patch, then only retained heads. Start
  with the documented 62-frame hop. Stream the recording, including its ending;
  account for padding and valid duration. Avoid a single central excerpt that misses
  a disruptive ending or climax. Model window estimates are not verified mood boundaries.
- Aggregate outputs incrementally using actual time support, accounting for overlap.
  Persist bounded means, variation, upper quantiles, coarse temporal coverage and a
  few useful support/contradiction intervals. Select the exact fields against the
  pilot's decision needs and the stage 3 byte budget. A brief climax must not silently
  describe the whole recording. Bound provider projections separately.
- Do not retain every embedding or window score across the library by default.
  Save full outputs only for selected pilot/debug fixtures when needed to diagnose
  aggregation or test a later classifier. Record exact manifests and valid intervals
  in those exports. Ordinary operation requires no NPY lifecycle, orphan-file cleanup,
  vector search, or second backup path.

**Gate:** reference parity, full/tail coverage, bounded long-file memory and payloads,
interruption/resume, stale-file races, cache hits and storage failure handling. Measure
seconds per audio minute, bytes per track and peak RSS. Test concurrent playback before
the full rebuild; reduce scheduling or reject the stack if the product budget is exceeded.

## 7. Improve the existing tagger using one shared evidence projection

**Owners:** application `mood_evidence`/decision types,
[tagger](../crates/music-application/src/assistant/model_tagger.rs), pilot tooling.

Create a bounded `song-evidence/v1` projection: allowed metadata, source-attributed
claims, learned musical predictions, temporal summaries, missingness and conflicts.
Exclude titles, filenames, paths, generated suggestions, manual tags and listening-test
labels. Include units, score meaning and source family; apply source-use permissions
before projection. Keep raw identities and embedding arrays local. Scope the common contract
to the current tagger and optional Jev.

Adapt the current structured-output tagger to this evidence and a per-tag support/
abstention result. Preserve its full-vocabulary route and eight-suggestion limit;
new evidence must not become hidden local preselection of allowed tags. Replace superseded
input/output parsers and fixtures. Version input, output, analyzer, disclosure, role
fingerprint and quality fixtures together. Do not
send evidence under the previous consent contract or register the old tagger as a fallback.

Compare baseline A with the updated tagger on development recordings. Use bounded
source-only, audio-only and combined ablations to establish which evidence helps.
Start with the most relevant failures; budget external calls before broad comparisons.
Retain an input or head only when its gain justifies extraction cost and complexity.
Test missing sources and contradictory passages; a consistent abstention is preferable
to a confident unsupported scene label.

A trained local classifier, learned source combiner and per-tag calibration are later
options, not requirements. Raw scores can be useful when clearly labeled and reviewed;
do not present them as validated probabilities or average unrelated provider scores.
Keep any initial decision thresholds fixed from development before confirmation.

**Gate:** contract/disclosure fixtures, deterministic projection, source withdrawal,
missing-data behavior and listening comparison. Retain useful catalog/DSP changes if
additional learned audio fails to improve the owner's results.
Custom training remains conditional.

## 8. Native Jev engine and listening comparison

Implemented on 26 September after the owner requested this alternative. Owners are
[typed decisions](../crates/music-application/src/assistant/typed_decisions.rs),
[Jev interpretation](../crates/music-application/src/assistant/model_jev.rs),
[native HTTP](../crates/music-server/src/provider_transport/typesafe.rs), the existing
provider setup and mood-tagging jobs. See the
[architecture contract](ASSISTANT_ARCHITECTURE.md) and
[operator workflow](../ASSISTANT.md#trying-jev-for-mood-tagging).

- Direct native `POST /v1/systemone`, strict Noul/Choice responses, exact version
  checks and `models[].name` discovery; pin `jev-1.13.0`. No chat/SDK bridge,
  generated-text capability, thinking/output setting or automatic retry.
- One song per state; group definitions and every runtime tag's full semantics are
  included. Partition all **200 allowed application tags** under both context limits.
  The separate offline pilot's larger vocabulary cap is not the runtime limit.
- Engine v4 uses a full period Choice plus an independent applicability Noul for its
  winner; other groups have one applicability Noul per tag. Complete definitions are
  inlined with each question. Choice probabilities and Nouls are not combined.
- Up to eight candidates receive independent support/contradiction Nouls for each
  observation. Multiple sources can support a tag without competing for probability.
  A mixed judgment is a conflict; at least one unambiguous support source is required.
  Fit, grounding and categorical selection have separate provisional 0.70 gates.
  Requests are bounded and fully reserved; oversized full period lists fail.
- The app assembles explanations, retains selected contradictions and labels every
  proposal tentative. Scores are uncalibrated. Exact request fingerprints and usage
  are checkpointed before cost; unchanged completed profiles use existing freshness.
  A separate response cache or inference store is unnecessary for this first pilot.
- Existing synthetic quality, live-data disclosure v16, review/export and transactional
  guards apply. Local fixtures cover malformed answers, full custom vocabularies,
  HTTP 401/422/429/529, submission timeout, budget rejection and restart uncertainty.

The native engine is available for comparison once configured; conformance passed on the owner's
deployed build, while live v3 quality failed. Engine v4 quality and independent
listening acceptance remain unverified. It consumes prepared textual evidence,
so it cannot supply musical information missing from that evidence. Do not loosen
safety or equate extra tags with quality to obtain a passing demonstration.

The implementation follows the official [API](https://docs.typesafe.ai/api),
[models](https://docs.typesafe.ai/models), [Noul](https://docs.typesafe.ai/primitives/noul)
and [Choice](https://docs.typesafe.ai/primitives/choice) contracts reviewed this date.

## 9. Integrate through existing review and authoring

**Owners:** [tagging jobs](../crates/music-application/src/assistant/model_jobs/tagging.rs),
[atomic review](../crates/music-storage/src/assistant/review.rs),
[Assistant HTTP DTOs](../crates/music-server/src/assistant/mod.rs),
[review UI](../frontend/src/views/assistant/AnalysisTagReview.tsx).

Keep the normal interaction: analyze selected music, review useful suggestions, accept
chosen tags, and use normal playlists. Provider configuration remains in AI setup.
Show concise per-tag support or uncertainty; put source, model, age, coverage and
contradictions in existing details/disclosures. Add a seek-to-evidence action only
if auditioning intervals saves effort. Do not create a second editor or permanent
research/diagnostics dashboard.

Recheck the complete decision identity during acceptance, including source policy,
features, vocabulary and engine revision. Only explicit review writes `track_user_tags`.
Keep generated evidence and accepted state separate so a model can be removed safely.
Use pilot exports and a documented listening protocol first; a dedicated annotation
screen needs repeated annotation work to justify it. Assisted corrections retain that
status and do not silently become independent confirmation labels.

Land the reset migration together with current writers/readers, generated HTTP types,
strict browser guards and new fixtures. Remove superseded analyzer registrations,
legacy fields/parsers, compatibility branches and old pilot entrypoints. Old clients
must refresh/update rather than receive an adapted old analysis response. Current
provider alternatives such as Jev implement the same new contract.

**Gate:** stale-review races, bulk acceptance atomicity, partial-source disclosure,
judgment isolation and focused frontend tests. Verify old payloads/jobs/review requests
are rejected and no old analysis route remains reachable. Applied migration history stays
intact; static research reports stay outside the runtime. No playback wire change is planned;
inspect/update Baton only if a consumed schema changes. No additional frontend state store.

## 10. Confirm benefit, cut over once, and rebuild the whole library

Choose a primary configuration using development results; freeze its manifests,
thresholds and comparison with baseline A before opening confirmation results. Jev
or another candidate can be evaluated separately if justified; do not require every
researched engine. A test-informed redesign needs fresh confirmation examples.

Report both kinds of outcome on the same recordings and requests:

- **Owner benefit:** suitable candidates found, time spent auditioning, disruptive
  false positives such as vocals/climaxes in a quiet bed, corrections and review time.
  Use the existing planner and ordinary playlist flow, with comparable request order.
- **Technical cost and quality:** per-tag precision/recall, useful-track coverage,
  abstention and judgment counts; analysis time, RAM/disk, setup burden and provider
  cost. Report uncertainty and known domain gaps; keep mood, scene and period separate.
  Add calibration metrics only for calibrated outputs. Use small source-removal or
  shuffled-audio controls when needed to establish what caused an apparent gain.

The pilot's 0.80 precision/0.60 coverage targets are starting thresholds. Seek a clear
gain on adequately judged core labels: for example, five percentage points more coverage
at matched precision, or equivalent quality with materially less review/cost. Confirm
workflow benefit; inconclusive results do not justify cutover or claims about all
138 labels. Drop models that add no value and retain independently useful fixes.

Validate the new pipeline on the pilot and representative folders before cutover.
Then stop old analysis workers, back up the database/authored files, deploy the contract/reset
change and supersede old analysis jobs before recovery. Rebuild every indexed recording
from original audio under the new contract, including previously successful tracks.
Recreate metadata/catalog-derived proposals from retained permitted source facts and
recompute local DSP plus every enabled voice, feature and interpretation stage. Refresh
source observations where policy/freshness requires it. No old result may satisfy the rebuild.

Use one resumable rebuild with visible pending/complete/failed counts; unavailable files
stay failed until accessible. New checkpoints can resume completed new-pipeline work.
Paid interpretation still requires the current disclosure and budget, with no automatic
retry of uncertain past attempts. Analysis failure leaves results unavailable; there is
no switch back to old analysis. Accepted tags change only through explicit review.

Verify reset/restart/restore, source withdrawal and preservation of authored data.
Measure concurrent playback under the container budget before the full rebuild. A model,
cache or provider failure must leave browsing, playback and authored playlists usable.
A later model/contract change invalidates affected generated results for reanalysis;
it does not add a reader for their old format.

Use logical local commits after focused checks; ship the destructive migration and
its matching runtime/UI changes as one coherent cutover. Runtime changes use
the applicable [validation matrix](VALIDATION.md): Rust and contract gates, frontend
checks for visible changes, dependency locks/license checks for new crates, and container
checks for inference packaging. Distinguish CI fixtures from paid-provider, licensed-model
and owner-listening validation. Reopen the conditional backlog only against an observed
shortcoming.

## Conditional work, with explicit reasons to add it

These are implementation options, not commitments or prerequisites. Use the admission
rule above before promoting one into the ordered plan.

| Observed need | Specific next implementation and boundary |
|---|---|
| Source/learned scores remain insufficient, or offline use/provider cost matters | Train independent L2 binary logistic heads for supported moods with [linfa-logistic](https://rust-ml.github.io/linfa/rustdocs/linfa_logistic/type.LogisticRegression.html), export scaling/coefficients, and run simple Rust inference. Start with existing scores/DSP; test embeddings only if needed. No multinomial softmax for coexisting moods. |
| A trained model or wider label claim needs more evidence | Expand the judged library cohort, potentially toward 400 distinct recordings; add grouped train/calibration/test partitions and secondary listeners where justified. Fit on judged labels only, mask unknowns, and keep confirmation protected. The sample count alone still cannot certify every tag. |
| Several useful sources cannot be combined reliably | Fit a small combiner on grouped out-of-fold predictions, with missing-source indicators; use separate held-out calibration if probabilities are needed. Never average vendor confidence or count correlated heads as independent votes. [Calibration methodology](https://scikit-learn.org/stable/modules/calibration.html). |
| The first stack misses useful soundtrack semantics | Compare [larger CLAP music](https://huggingface.co/laion/larger_clap_music) upstream before native integration: deterministic ten-second windows/five-second hops including the ending, 48 kHz decoding, cached text embeddings for tag definitions/confusable alternatives. Preserve preprocessing while replacing its default random truncation; similarity is not calibrated probability. [Processor](https://huggingface.co/laion/larger_clap_music/blob/main/preprocessor_config.json). |
| Repeated inference or a concrete training/retrieval feature needs reusable tensors | Measure cost first. Then add encoded/decoded-PCM identity with decoder revisions, separate encoder/head keys, or bounded NPY artifacts via [npyz](https://docs.rs/npyz/latest/npyz/), whichever addresses the need. Include quota, atomic publication, corruption recovery and backup/regeneration. Do not introduce all three automatically. |
| An independent source-domain question cannot be answered with local examples | Add a narrow manifest/annotation importer: [MTG-Jamendo](https://github.com/MTG/mtg-jamendo-dataset) for weak mood labels, [DEAM](https://cvml.unige.ch/databases/DEAM/) for timestamped valence/arousal, or [OpenMIC](https://github.com/cosmir/openmic-2018) for masked instrument labels. Preserve scope, unknowns, official splits, overlap and licensing. MTG terms/audio licenses require separate review; training-set performance is not independent validation. |
| Affect dimensions remain weak | Probe `msd-musicnn-1` plus `deam-msd-musicnn-2`; its [head](https://essentia.upf.edu/models/classification-heads/deam/deam-msd-musicnn-2.json) expects 200 values and outputs valence/arousal, incompatible with EffNet or the voice classifier's two scores. Keep only if session/listening results improve. |
| Rhythm remains a material selection failure | After the stage 5 repair, probe [Beat This!](https://github.com/CPJKU/beat_this) with no-beat/rubato and resource gates. A standalone beat-analysis product is outside scope. |
| A missing catalog fact changes a useful decision | Add the specific MusicBrainz work relationship, Discogs credit, or referenced Wikidata claim through existing typed adapters. Preserve version/entity scope; lyrics require a separately permitted text channel. Avoid collecting fields without a consumer. |
| Manual pilot management repeatedly becomes a bottleneck | Extend pilot automation or a compact blind-listening mode, then consider active-learning selection with a random audit fraction. Preserve grouping and frozen tests; do not build a general dataset product. |
| The smaller choices fail an important use case | Compare [MuQ-MuLan](https://github.com/tencent-ailab/MuQ) or [Cyanite](https://docs.cyanite.ai/docs/intro/) against that failure before integration. Account for larger runtime/weight terms or explicit audio-upload consent and recurring cost. |

Large-encoder fine-tuning, training on generated tags, several production analysis
services, and a vector database have no demonstrated first-delivery requirement.
Keep the researched alternatives available without turning them into product scope.
