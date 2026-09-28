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

## Private Jev input-minimization pilot results: 2026-09-28

The owner approved execution of the prepared six-recording comparison. All **20/20
full native tagging executions** completed: three input variants for each of six
recordings, plus two deliberate current-input repeats. Fresh native conformance
passed. The run made **73/73 successful requests**, all reporting pinned
`jev-1.13.0` and usage, with no retries, unresolved attempts or missing results.

Every execution produced an explicit **empty final tag list**. All 130 non-period
definitions were below the unchanged 0.70 initial-match threshold. Each period
Choice included all eight configured periods and selected `no_supported_period`. The highest
non-period match across the run was 0.58. No candidate qualified for follow-up
grounding, so the complete task finished with abstention; these are not transport
failures, unavailable profiles or a first-pass-only substitute for final outputs.

| Input variant, six primary executions | Requests | Reported input tokens | Reduction against current |
|---|---:|---:|---:|
| Current v9 | 23 | 309,177 | Baseline |
| Lossless initial-state compaction | 23 | 293,562 | 5.05% |
| Compaction without section observations | 18 | 242,304 | 21.63% |
| Two current-input repeats | 8 | 107,664 | Control; excluded from paired savings |
| Synthetic conformance | 1 | 476 | Setup; excluded from paired savings |

Total observed usage: **953,183 input / 52,418 output tokens**, approximately
**US$0.0400** at the verified [$0.042/M input rate](https://docs.typesafe.ai/models),
with output free. The durable native ledger reserved 3,885,118 conservative units,
within the approved 329-request / 14,721,150-unit cap. Savings above apply to these
abstaining runs; they do not establish quality equivalence when tags trigger grounding.

Exact final per-recording outputs, actual wire requests, diagnostics and the listening
sheet remain private under `target/jev-listening-20260928/run/`. All six source
hashes were rechecked after execution and remain unchanged. An independent offline
reconciliation verified every attempt/response, model ID, usage total, selected
recording, final profile, diagnostic decision and empty tag list. The owner has not
provided listening judgments; no precision, recall or quality pass is claimed.
Production remains v9 with unchanged thresholds and acceptance gates; no authored
tags were changed. The sixteen unselected recordings were not sent to Jev.

Plan SHA-256: `7c9fa155e3a0e25447fe20e5cacb8eddc5e105e85d729cd69e58ec4a6f3c901d`.
Journal SHA-256: `d7260312ad1c29c0126a36e23547108b76751fcffd682271ae2d55d203126f39`.
Result SHA-256: `f10fe5291098293421a2acd3ece937a2e593828f4d551955607df2795b952e02`.
This completes the approved private pilot. Further paid experiments require their
own reviewed scope and budget. Owner judgments remain the next input to optimization.

## Private Jev input-minimization pilot prepared: 2026-09-28

Following the completed synthetic screen, the owner authorized read-only use of two
local album folders and requested actual suggestions to rate. All **22 recordings**
were analyzed locally, covering **91.16 minutes** of audio in **110.5 seconds** of
reported analyzer work. The native metadata reader and ffprobe both found no embedded
artist, album or genre descriptions. Filenames and collection names remain local
listening aids and are excluded from provider input; no catalog/voice model was added.

Six recordings were frozen before predictions: three per collection, selecting the
lowest and highest typical rhythmic-drive proxy and a third with greatest minimum
distance on range-normalized drive, brightness, density and spectral-flux values.
Ties use local track ID. This intentionally contrasting development sample is not
representative accuracy evidence; sixteen recordings remain unexposed. The selection
and content hashes are recorded privately under `target/jev-listening-20260928/`.

| Initial-assessment input | Requests, six recordings | Repeated state bytes | Question bytes |
|---|---:|---:|---:|
| Current v9 | 23 | 240,344 | 950,099 |
| Lossless compaction | 23 | 181,647 | 950,099 |
| Compaction with section observations omitted | 18 | 49,056 | 950,094 |

These are serialized bytes, not measured provider tokens or quality gains. Questions
account for **79.8%** of baseline assessment bytes. Compaction reduces combined
assessment state/question bytes by **4.9%**. Omitting sections reduces that measure
by **16.1%**, changes native packing, and also removes section grounding work; its
full-pipeline reservation is **50.8% lower** before model-dependent follow-ups.
Required tag coverage remains all 138 default vocabulary entries. No label or
criterion was shortened or removed. The five-byte question-total difference is
serialization overhead from different partitions, not changed question content.

The bounded developer pilot reuses native selection, independent support/conflict,
period applicability and output validation. It adds initial-state compaction only in
the experiment transport; the omission arm removes sections from both matching and
grounding while retaining trajectories, endings, coverage and reliability. The source
corpus and production implementation remain intact. Fresh synthetic conformance and
two deliberate identical-input repeats are included. Output is assisted development
feedback, not blind listening, a new full-suite result or application certification.

Prepared cap: **329 requests / 14,721,150 conservative input units**, pinned
`jev-1.13.0`, no retries; approximately **US$0.62** at the published input rate.
Plan SHA-256: `7c9fa155e3a0e25447fe20e5cacb8eddc5e105e85d729cd69e58ec4a6f3c901d`.
At preparation, no paid call was implied and the prior synthetic allowance was
consumed. The owner subsequently approved this exact budget; execution is recorded
above. See the
[developer pilot procedure](MOOD_PILOT.md#developer-jev-input-comparison).

## Jev evidence-source and amount results: 2026-09-28

The approved experiment completed **1,616/1,616 requests** from `d8c590c` against
pinned `jev-1.13.0`, with **19,158,055 input / 1,290,285 output tokens**. Every
attempt has one response; there were no retries, unresolved attempts or missing
usage counts. All 1,471 logical pairs are complete. At the verified
[$0.042/M input rate](https://docs.typesafe.ai/models), the estimated charge is
**US$0.8046**, with output free, within the approved 82,194,553-unit allowance.
This is an input-cost calculation, not an invoice or a new quality certification.

**Decision:** lossless card compaction is the preferred next candidate for full
validation. Retain complementary album and descriptive genre evidence, retain
temporal trajectories, and avoid repeated evidence or a blanket addition of
identity fields. No production projection, inference identity, vocabulary, fixture,
threshold or acceptance state changed. Historical full quality remains 55/66;
these first-pass candidate counts do not replace that result.

### Complete paired screen

Each row compares its own matched baseline, not another row's denominator.
"Required" counts non-period tag assignments crossing the unchanged 0.70 initial
fit threshold, before capacity and grounding. Different cases and variants can
share a physical request; the report exposes all shared indices and expectations.
Paired token changes are relative to that row's baseline and must not be added
together as total spend. No explicit forbidden Noul candidate crossed 0.70 in any
arm. This does not certify final tags, period applicability or the full safety gate.

| Input variant | Paired cases | Required retained: baseline -> variant | Gained / lost | Paired input tokens |
|---|---:|---:|---:|---:|
| Compact cards, all facts retained | 64 | 139/151 -> 142/151 | 3 / 0 | -0.68% |
| Three copies of the same cards | 64 | 139/151 -> 128/151 | 2 / 13 | +7.34% |
| Add artist/origin/duration/unverified BPM | 58 | 135/147 -> 134/147 | 2 / 3 | +1.22% |
| Album only | 55 | 138/147 -> 80/147 | 3 / 61 | -1.39% |
| Genre only | 55 | 138/147 -> 71/147 | 0 / 67 | -1.39% |
| Genre text presented as catalog data | 55 | 138/147 -> 128/147 | 3 / 13 | +0.85% |
| Genre text presented as a community label | 55 | 138/147 -> 125/147 | 1 / 14 | +0.90% |
| Metadata only, mixed-source cases | 3 | 6/8 -> 7/8 | 1 / 0 | -14.06% |
| Audio only, mixed-source cases | 3 | 6/8 -> 0/8 | 0 / 6 | -0.77% |
| Without sections, trajectories retained | 11 | 7/12 -> 9/12 | 2 / 0 | -3.68% |
| Without trajectories, sections retained | 11 | 7/12 -> 6/12 | 0 / 1 | -8.20% |
| Without voice card | 11 | 7/12 -> 8/12 | 1 / 0 | -0.85% |
| Numbers without derived bands | 11 | 7/12 -> 8/12 | 1 / 0 | -4.43% |
| Bands replacing binned magnitudes | 11 | 7/12 -> 7/12 | 0 / 0 | -1.98% |
| Unchanged-input repeat control | 6 | 11/15 -> 12/15 | 1 / 0 | 0.00% |

### What the data supports

- **Musical descriptions and setting descriptions do different jobs.** Within
  the metadata-only cohort, combined evidence retained 61/62 required mood
  assignments; genre-only retained 59/62 and album-only 9/62. For settings the
  corresponding counts were 30/33, 1/33 and 30/33; for scenes, 37/40, 6/40 and
  34/40. These fixtures put rich phrases such as "jubilant folk for a holiday
  celebration" in `genre`. They support retaining descriptive musical content,
  not a claim that a bare genre label predicts mood or that field names determine
  meaning. Album descriptions supply useful tabletop-use context in these cases.
- **Compaction has the best observed tradeoff, with limited evidence of gains.**
  It preserves all values, timing, reliability, source attribution and ending data,
  removes redundant card IDs and shares the repeated band explanation. Its gains
  were Arctic 0.68 -> 0.71, escape 0.66 -> 0.76 and festive 0.66 -> 0.70. The 18
  repeat-control requests produced 654 Noul comparisons, median absolute movement
  0.00 and maximum 0.06; Arctic crossed 0.68 -> 0.70 with no input change. Thus two
  compaction gains fall within observed repeat variation. The escape change is
  larger, but this small control set is not a confidence interval or proof of
  statistical significance. Compaction reduced paired input tokens only 0.68%;
  its main rationale is simpler evidence presentation, not substantial cost savings.
  Mood-only retention moved 65/70 -> 66/70; festive was its sole mood gain.
  Triplication left mood at 65/70, with all thirteen losses in setting/scene tags.
  This screen does not demonstrate a robust improvement in mood recognition.
- **Temporal change is useful evidence.** Removing trajectories dropped the
  acoustic volatile-development case's `chaotic` score from 0.74 to 0.26. Keep
  dynamics and end-state summaries. Removing section cards recovered escape
  (0.66 -> 0.77) and festive (0.66 -> 0.70), while retaining trajectories including
  their ending. This motivates a separately tested smaller initial-match view;
  it does not justify deleting sections from local analysis or conflict review.
- **The current physical proxies leave a semantic gap.** Removing metadata from
  the three mixed cases lost all six previously retained required assignments.
  No tested variant recovered the unresolved acoustic-only calm or urgent
  expectations: the settled input stayed below 0.70 (at most 0.68), and sustained
  drive reached at most 0.42 for urgent. The two settled fixtures share identical
  projected evidence and are not independent confirmations. Renaming numeric
  ranges as bands did not repair these misses. The expectations still need
  independent listening qualification; the experiment neither proves that the
  measurements establish those emotions nor that audio analysis is unnecessary.
- **Extra context is not automatically helpful.** Triplicated evidence gained
  two required candidates but lost thirteen. Adding identity fields gained two
  and lost three. Keep identity for source matching; neither variant warrants
  broadening initial tag matching. The source-relocation probes also regressed,
  but contain copied synthetic phrases, not actual retrieved catalog records.
  Do not relabel sources to inflate support or infer MusicBrainz/Last.fm retrieval
  quality from this test. Preserve each observation's real source and scope.

Period Choice was analyzed separately. Compact cards changed one low-confidence,
unrequired relative winner in the castle-procession negative case (industrial
0.35 -> medieval 0.37); no independent applicability Noul or grounding ran.
There is no demonstrated period-tagging improvement from compaction.

The 58 default-vocabulary cases averaged **38,518 initial-match input tokens**
(median 37,351; range 37,330-45,594), about **US$0.00162 per case** at the same
rate. These are logical per-case totals using the experiment's common partitions;
shared requests are reused in actual spend. They exclude later support/conflict
checks and cannot be presented as total production song-tagging cost. The five
small custom-vocabulary cases averaged 1,447 input tokens; the one maximum-size
case used 52,191. Vocabulary/question coverage is a major part of cost.

### Next implementation sequence

1. Promote only the exact lossless compaction into the initial assessment state
   projection in `crates/music-application/src/assistant/model_jev.rs`, leaving
   source facts and grounding inputs intact. Advance inference identity; test
   reconstruction, missingness, provenance, ending data and stale-result rejection.
2. Prepare and separately authorize full native conformance, all 66 scenarios and
   safety repeats using actual production packing. Keep the 90% quality gate and
   all safety requirements. Do not combine compaction with untested omissions or
   describe its 142/151 first-pass count as full acceptance.
3. If the remaining evidence burden warrants it, test a separate initial-match
   view retaining album/genre, trajectories, coverage and reliability while keeping
   sections available for grounding and local use. The combined compact/filtered
   view has not been tested, and the mixed-source sample has only three cases.
4. Qualify the calm/urgent acoustic expectations through the existing independent
   listening pilot. Only then evaluate whether the already-probed matched
   Discogs-EffNet/MTG-Jamendo semantic heads supply missing useful evidence. Avoid
   adding more raw features, a new annotation platform or a source combiner without
   a measured need and suitable independent labels.

Artifacts are local and ignored: `target/jev-evidence-results-20260928.jsonl`,
`target/jev-evidence-report-20260928.json` and the independent arithmetic summary
`target/jev-evidence-analysis-20260928.json`. The validated report matches plan
`d539166ea17681eb404591b0aec65ba5ed0f4a285bbe3639b00a957b15defbee`.
Journal SHA-256: `119f069c8efb8c9cda2ca03c109da62329a44e876d25e0059ff599ab248eb701`.
Report SHA-256: `ab2dcd112879a0729503f43b97e8cf49168a2a95ddbf8373a1cb7167f00f6e8e`.
The current engine remains v9; no new production model certification is recorded.

## Jev evidence-source and amount experiment prepared: 2026-09-28

The owner requested controlled additions, omissions and input-size comparisons.
The developer runner now prepares **14 data variants plus current input** across
all 66 unchanged synthetic cases. Questions, explicit criteria, definitions,
thresholds and expected tags stay fixed. The two cases with no eligible production
evidence remain accounted for without calls. At preparation no provider run had
been made; the completed results and qualified conclusions are recorded above.

| Variant | Question investigated |
|---|---|
| Metadata only | Does adding the available audio context help or distract? |
| Album only / genre only | Which description supplies support, and does combining them help? |
| Audio only | What can the existing measurements support without descriptions? |
| Without sections | Do local changes add value beyond trajectories, which still include their ending? |
| Without trajectories | Do aggregate measurements add value beyond sections, which retain the ending? |
| Without voice | Does the optional voice estimate affect non-vocal tagging? |
| Numeric only | Do descriptive physical bands help beyond raw values? |
| Bands only | Does replacing binned magnitudes with words help? Timing and reliability remain. |
| Compact cards | Can redundant IDs and repeated scale explanations be removed without losing facts? |
| Three copies of the same cards | Does additional, explicitly duplicated detail change judgments? |
| Genre as catalog / community label | How does source presentation affect the same text? These are synthetic relocation probes, not newly verified facts. |
| Add supplied identity context | Do artist/origin/duration/unverified BPM distract? Paths, filenames and new descriptions are never added. |

All retained observation meanings, reliability, coverage, missingness and conflict
values remain, except the explicitly named ablations. In particular, compact cards
are reversible and preserve ending data; a shorter input is not an automatic win.
Metadata-only variants retain the original coverage qualifiers. The source probes
preserve the exact original genre phrase in production-shaped attributed cards;
they cannot establish real MusicBrainz/Last.fm retrieval quality or independent
corroboration. Ordinary catalog genres are not verified mood labels.

Partitions are sized for the largest state and then shared across all arms for a
case. This creates 194 logical current-input partitions instead of the normal 183;
the question bodies themselves remain identical. Arm order rotates deterministically.
Six cases receive one separately budgeted identical-input repeat, totaling 18
control requests; this exposes observed variation but is not a significance test.
There are 349 unchanged/inapplicable case variants and 80 empty-data variants with
no calls. Empty variants are not successes or zero-valued model answers.

Exact provider bodies are deduplicated across cases and variants, except the 18
deliberate controls. This saves 49 requests and 2,114,805 reserved units without
dropping any of the 1,665 logical uses or 1,471 paired comparisons. There are 46
shared requests, exposed with their uses and distinct case-expectation sets.
Removing distinguishing evidence can leave identical inputs with different
expectations. One answer is then scored against each case's own expectations;
these uses are not independent observations. Current input needs 190 physical
requests after deduplication. The first draft was not executed.

Prepared plan: **1,616 requests / 82,194,553 conservative input units**, pinned
`jev-1.13.0`, synthetic text/JSON only, no retries. Plan SHA-256:
`d539166ea17681eb404591b0aec65ba5ed0f4a285bbe3639b00a957b15defbee`.
Local artifact: `target/jev-evidence-plan-deduplicated-20260928.json`. At the published
[$0.042 per million input tokens](https://docs.typesafe.ai/models), treating every
reserved unit as a billed token is approximately **US$3.45**; actual usage is
expected to be lower and must be read from responses. Output is listed as free.
This separately reviewed budget was explicitly approved before execution;
previous completed-run approvals were not reused.

Offline coverage inspection finds **52 metadata-only, nine audio-only and three
mixed cases** among the 64 that make requests. None contains existing catalog
cards. Consequently, text/audio interaction findings will initially cover only
three synthetic cases, and source attribution probes cannot fill that dataset gap.
In the preceding production-shaped baseline, evidence occupies roughly **0.58%,
6.57% and 8.08%** of serialized state-plus-question bytes in those respective
cohorts. These are byte measurements, not token/cost predictions; repeated questions
and tag definitions dominate those inputs.

The offline report separates required-tag retention, explicit forbidden candidates,
unscored changes, paired coverage, tag groups, source cohorts, state bytes, actual
usage and identical-input controls. Required-tag losses after removing useful data
can represent appropriate abstention. Logical paired counts and usage are separate
from distinct request sets; only root usage counts each paid response once. Neither
paired baseline nor variant usage may be summed as total spend. Missing
responses/counts remain unknown. No automatic
winner, final scenario score or musical-accuracy claim is produced. Period Choice
winners and scores are recorded separately from Nouls; their independent
applicability check, grounding, candidate capacity, conformance and safety repeats
are outside this screen. A promising combination requires full native validation and
independently judged recordings, rather than tuning thresholds to this suite.

Production remains v9, with no runtime, vocabulary, fixture, provider dependency,
quality threshold or acceptance change. The existing no-criteria experiment and
full native quality runner retain their exact contracts. The shared checkpoint
validator now serves both developer comparisons. Local validation is recorded in
the implementation plan; the separately approved live results are recorded above.

## Jev optional-criteria comparison rejected: 2026-09-28

The separately authorized comparison completed **366/366 requests** against pinned
`jev-1.13.0` from commit `9316158`. Removing initial-fit criteria recovered **no**
required assignment, lost **four**, and introduced **one** forbidden first-pass
candidate. Reject this variant; there is no reason to spend on its full validation.
Production remains v9 with explicit criteria. Historical v7 quality is **55/66**;
this first-pass comparison is not a new full-suite result or certification.

The adapter previously required Noul criteria even though TypeSafe documents them
as optional and recommends comparing questions with and without them
([Noul guidance](https://docs.typesafe.ai/primitives/noul)). It now accepts omission
while rejecting incomplete explicit criteria; Choice still requires options.
Optional criteria remain a supported API form, not a recommended music-tagging
configuration. The results do not support removing them as a repair for the misses.

The new comparison changes **only initial-fit Noul criteria**, retaining exact
production state, instructions, definitions, thresholds, Choice questions and
partitioning. All 66 unchanged suite cases are accounted for: 64 have requests and
two have no eligible evidence. It compares **7,760 paired Noul judgments**, including
**151 required assignments**, every previously passing positive lost in v8, custom
definitions and all applicable safety controls. Period Choice is unchanged and
not scored by this diagnostic. Grounding and safety repeats require the full run.

Approved and executed plan: **366 requests / 17,016,148 conservative input units**,
synthetic data only, no retries. SHA-256:
`d8a3642bb41e4d264545183351af8786dfb59289835f4a7c7dcee69133666c0f`.
The rebuilt plan matched this exact identity before the key was read. All 183
partitions have both responses, every response reports usage, and there are no
unresolved attempts. Actual usage: **3,973,016 input / 311,830 output tokens**.

| Initial Noul gate, before grounding | Current criteria | Without criteria |
|---|---:|---:|
| Required assignments at or above 0.70 | 139/151 | 135/151 |
| Explicitly forbidden assignments at or above 0.70 | 0/976 | 1/976 |
| Reported input tokens, 183 requests each | 2,273,628 | 1,699,388 |

| Lost required assignment | Current fit | Without criteria |
|---|---:|---:|
| Quiet intro, urgent escape / chase | 0.75 | 0.54 |
| Slow, intense siege / combat | 0.77 | 0.58 |
| Fast, light market dance / dancing | 0.81 | 0.67 |
| Volatile acoustic development / chaotic | 0.73 | 0.68 |

The custom `dark` definition in `custom-vocabulary-alias` rose from **0.69 to 0.79**.
Here it means dim, warm, reassuring ambience, not horror. The supplied lamplit-study
description does not establish all of those properties. This is a forbidden
candidate flag; grounding was not run, so it is not an observed final false tag.
There are also 27 unscored crossings (18 up, nine down), which cannot be counted as
either improvements or errors. The smaller input bill does not justify the losses.

None of the twelve baseline misses was recovered. The clearer descriptive cases
remain below the gate: Arctic **0.63 -> 0.67**, temple **0.67 -> 0.58**, festival
**0.66 -> 0.65**, and storytelling **0.39 -> 0.26**. The current baseline also puts
quiet-intro `escape` at **0.69**, compared with **0.70** in the historical v7 full
run. A single paired run establishes the observed rejection decision, not a
repeatability estimate or an explanation for every score change.

Local evidence: `target/jev-criteria-plan-20260928.json`,
`target/jev-criteria-results-20260928.jsonl`, and
`target/jev-criteria-report-20260928.json`. Journal SHA-256:
`2781408ac08a39a4fdf2ab999d0dcf56c87e98105e8f19026f910219ca57a2b5`;
report SHA-256:
`45e4ed40278aaa9b0d2b88baba47382a2df0c698bdc70e89a1b431fc9fa19dec`.
The approved run is complete; its authorization does not cover another run.

The offline journal report lists required gains/losses, new forbidden candidates,
resolved forbidden candidates and unscored threshold crossings. An unlisted tag
is not automatically a negative, and a first-pass forbidden crossing is not yet a
false-positive final tag. Missing/unpaired responses remain unknown; a stopped or
interrupted journal cannot appear complete. No diagnostic pass rate or model
certification is issued. The report was also exercised through its CLI with a
simulated interrupted journal, without credentials or network access.

Semantic inspection retains the distinction between clear descriptive misses
(Arctic, temple, festival and probable storytelling), textual boundaries
(city/shopping and court festivities), and acoustic benchmark assumptions. The
two settled-texture cases have identical provider-visible musical state after
loudness filtering; their different historical scores do not establish an audio
sensitivity. High steady drive alone does not verify time-critical urgency.
No vocabulary, fixture or threshold was changed to resolve those ambiguities;
independent listening remains necessary.

Next, inspect the supplied propositions and evidence cards behind literal metadata
misses before preparing another single-factor comparison. Retain passing positives,
custom definitions and explicit negatives; resolve ambiguous expectations separately
from acoustic listening acceptance. Do not lower thresholds, add a keyword fallback,
or expand data collection to explain failures on already explicit descriptions.

Run preflight: all **eight** experiment tests pass and the regenerated plan matches
the approved hash and both caps. The completed journal passes the offline reporter;
the dated-record diff and all **152** checked local links pass. No runtime code,
fixture, vocabulary, threshold, production configuration or acceptance state changed.

Preparation validation had passed **582 Rust tests** with real FFmpeg and the pinned
voice model, **eight** developer-example tests, nine focused typed-adapter tests and
seven architecture policy tests. Formatting, architecture, workspace check,
strict workspace Clippy, doc tests and generated contracts passed. All 165 local
links in that preparation's checked Assistant references resolved. No dependency or fuzz source
changed in that preparation; neither batch establishes listening or production acceptance.

## Jev full v8 validation and rollback: 2026-09-28

The authorized full run completed conformance, all unchanged v29 scenarios and
all safety repeats. V8 regressed to **49/66 scenarios (74.2%)** and **140/159
required assignments**, against v7's 55/66 and 148/159. Safety remained **16/16**,
custom vocabulary **5/5**, maximum vocabulary **1/1**, and acoustic/context-only
**6/9**. No previously failing scenario improved. All nineteen missing assignments
were rejected at initial fit, before grounding or candidate capacity.

| Previously retained assignment | V7 full-run fit | V8 full-run fit |
|---|---:|---:|
| Forest hunt / hunting | 0.79 | 0.67 |
| Arctic escape / escape | 0.76 | 0.67 |
| Infernal ritual / infernal realm | 0.89 | 0.55 |
| Curious puzzle / puzzle | 0.73 | 0.69 |
| Temple band-name control / city | 0.74 | 0.66 |
| Quiet intro, urgent escape / escape | 0.70 | 0.60 |
| Quiet intro, urgent escape / chase | 0.77 | 0.67 |
| Slow, intense siege / combat | 0.74 | 0.63 |

The 28-request diagnostic covered selected failures and negatives, but omitted
these passing tag judgments. Its promising boundary scores also did not hold:
Arctic reached only 0.69 and temple 0.68 in the full run. This is insufficient
evidence to adopt the wording. The changed question family accounts for every new
omission; the full run does not isolate which individual clause caused it. TypeSafe
[documents independent questions](https://docs.typesafe.ai/primitives), so request
batching is not an established explanation. Future comparisons must include
previously passing positives, failures and negative controls, then pass the full suite.

**Engine v9 removes the v8 place/activity rewrite and restores v7's questions.**
The broader inference fingerprint and exact-budget runner checks remain. There is
one current engine, with no legacy fallback. Definitions, evidence, thresholds,
fixtures and review requirements are unchanged. The historical 55/66 result is
not a fresh v9 certification; quality repair and independent listening remain open.
The dimension-predicate comparison was rejected; its saved journals remain
historical evidence, not a recommended new paid run.

All **287 requests**, including conformance, received responses: **3,358,024 input**
and **214,363 output tokens**, with **14,155,387 conservative units reserved**.
This stayed within the approved 818-request / 20,172,234-unit plan, SHA-256
`80498adc9cc093609cac0e62e663ed6c174ad540d711856efc7014f6ad812ef2`.
There were no retries, uncertain requests or application acceptance writes.
Local evidence: `target/jev-quality-v8-20260928/result.json`; comparison baseline:
`target/jev-quality-v7-20260928/result.json`. No additional paid run is authorized.

Rollback validation passes: **581 Rust tests**, with real FFmpeg and the pinned
voice model, four developer-example tests and sixteen architecture/workflow policy
tests. Formatting, architecture, workspace check, strict workspace/fuzz Clippy,
doc tests, generated contracts and both graphs' deny/audit/machete checks pass.
An offline comparison confirms all **66 assessment plans**, conformance, scoring
expectations and request/input bounds match v7 exactly; only engine/fingerprint
identity changes. The live request builders also match v7. All 165 local links in
the edited references resolve. These checks establish restoration, not listening quality.

## Jev first-pass investigation: 2026-09-28

The owner's v7/v29 export (`07c24222ba4c4e6d8425c3aeb4bb3346`) reproduces the
previous local result: **55/66 scenarios**, **148/159 required assignments**,
**16/16 safety**, **5/5 custom**, **1/1 maximum vocabulary**, and **6/9 context-only**.
All eleven omissions fail initial matching; none fails grounding or candidate
capacity. All 288 quality requests received responses, using 3,253,887 input and
216,455 output tokens. Separate conformance used 476 input and 147 output tokens.

| Missing required assignment | Initial fit | Boundary to investigate |
|---|---:|---|
| Arctic escape / arctic | 0.67 | Explicit setting description |
| Court intrigue / city | 0.22 | Explicit setting description |
| Market day / shopping | 0.56 | Activity implied by a market description |
| Bard competition / festival | 0.68 | Described festival, not literal combat |
| Campfire story / storytelling | 0.42 | Explicit activity description |
| Modern devotions / temple | 0.65 | Explicit setting description |
| Light market dance / festive | 0.68 | Musical merriment versus event association |
| Renaissance masquerade / festive | 0.39 | Event description does not clearly establish musical mood |
| Settled acoustic texture / calm | 0.67 | Broad character inferred from numerical proxies |
| Same texture, louder master / calm | 0.64 | Same musical evidence; gain must not determine mood |
| Sustained acoustic drive / urgent | 0.31 | Broad character inferred from numerical proxies |

The v7 first-pass builder gives setting and scene questions a mood-oriented
positive criterion and an emotion-specific rejection rule. Their group scope says
to judge tabletop use, so these instructions disagree about what is being judged.
An authorized, hash-bound comparison isolated dimension-specific predicates for
setting, scene and mood, and made disjunctive definitions explicit without dropping
required qualifiers. Definitions, synonyms, evidence, scopes, conflicts and custom
predicates stayed fixed. Fifteen cases included metaphor, geography-to-emotion,
injection, redefined-label and contradictory-ending controls. Identical custom
variants were deduplicated: **28 requests**, reserving **469,750 conservative input
units** within the approved 470,610-unit cap. Every request received a response;
reported usage was **119,043 input** and **5,896 output tokens**, without retries.

| Initial matching | Current v7 | Dimension predicate |
|---|---:|---:|
| Arctic | 0.66 | 0.70 |
| City | 0.21 | 0.30 |
| Shopping | 0.59 | 0.65 |
| Storytelling | 0.43 | 0.53 |
| Festival | 0.67 | 0.67 |
| Temple | 0.62 | 0.70 |
| Acoustic calm | 0.67 | 0.57 |
| Acoustic urgent | 0.35 | 0.28 |

Selected metadata support independently improved or held: Arctic album 0.74 to
0.89, city 0.37 to 0.45, shopping 0.77 unchanged, storytelling 0.67 to 0.84,
festival 0.74 to 0.81 and temple 0.86 to 0.88. Combat metaphor fell from 0.26 to
0.14 at matching; injected combat remained 0.08/0.09 and geography-to-cold remained
0.24/0.29. The rewrite promoted no selected negative across the 0.70 threshold.
The unchanged custom low-light control still needs grounding to reject `quiet focus`:
its fit is 0.77 but album support is 0.53. These are selected judgments in a
controlled diagnostic, not full scenario outcomes.

**Engine v8 adopts only the measured setting/scene predicates for initial fit and
album/genre support.** The mood rewrite is rejected because it worsened acoustic
matching. Mood, period, custom, acoustic/catalog support, conflict questions,
evidence and all thresholds retain their preceding behavior. The inference identity
now includes each group-specific question family, invalidating stale checks/results.
Regression checks bind production use questions to the exact measured variant.
An offline comparison against the paid journal confirmed 33 exact measured
replacements, 113 unchanged questions and identical evidence for all fifteen cases.
Already adopted variants are omitted from future comparison plans.

The Renaissance fixture's event-to-mood ambiguity and the three acoustic expectations
remain unresolved. City's definition concerns dense urban/metropolitan life, while
its album describes a royal city; whether the broad location alone satisfies that
definition needs a vocabulary decision. Do not silently inject display labels,
weaken thresholds, or rewrite evidence to satisfy the expected tag. All v29 inputs,
required/forbidden tags and quality gates remain unchanged.

Fresh v8 full validation was prepared and subsequently authorized: conformance plus
all 66 scenarios and 16 safety repeats, capped at **818 requests / 20,172,234
conservative input units**. It regressed and was rejected, as recorded above.
The developer runner's hard input ceiling is
21 million to accommodate the longer measured prompts; caller caps must equal
the reviewed plan's exact totals, and its hash remains mandatory. Tests reject
both smaller and larger caps, over-ceiling and changed-plan calls before credentials are read. This does not
authorize another run. Full quality and independent listening remain open.

Local comparison evidence: `target/jev-dimension-results-20260928.jsonl`.

Engineering validation passes: **581 Rust tests**, no skips, with real FFmpeg and
the pinned optional voice artifact; four developer-example tests; formatting,
architecture, workspace check, strict workspace/fuzz Clippy, doc tests and generated
contracts. No frontend behavior changed. These checks do not certify model quality.

## Jev metadata support repair: 2026-09-28

The owner's subsequent v6/v28 export (`dad2a539b42b419a8fa47cc727952fa2`)
passed **45/66** scenarios, **16/16** safety checks and **6/9** context-only
cases. All 288 quality requests received responses: 3,249,908 input and 215,769
output tokens. It missed 22 required assignments: 16 at matching and six at
grounding, with no candidate-limit loss. The custom low-light case also returned
`quiet focus` without evidence of reading or study. This confirms the previous
run's failure pattern rather than a provider or transport outage.

Corrected four proven fixture contradictions in suite **v29**: positive tavern,
ruins, village and temple cases now describe their settings in the supplied album
field. Previously only the excluded `origin` field established those settings.
All required/forbidden labels, 66 cases, 16 safety repeats, vocabulary/context
subgates and the 90% threshold remain. The injection prefix and provenance-only
negative remain; paired native regressions prove retained artist/source names
cannot trigger inference after album and genre descriptions are cleared.

An explicitly authorized **30-request** comparison held fifteen synthetic inputs,
two tags per case, observation content and conflict questions fixed. It compared
current versus direct-semantic fit/support questions and used **119,096 input**
and **6,160 output** tokens, within 475,872 conservative reserved input units.
All requests completed without retries. Selected album-support observations:

| Case/tag | Current support | Direct semantic support |
|---|---:|---:|
| Calm travel / calm | 0.64 | 0.85 |
| Demonic rituals / ritual | 0.66 | 0.90 |
| Solemn coronation / majestic | 0.41 | 0.73 |
| Rebellion / defiant | 0.62 | 0.93 |
| Warm low light / unsupported quiet focus | 0.71 | 0.52 |
| Warm low light / correctly redefined dark | 0.94 | 0.98 |

The same broad rewrite worsened initial Arctic matching (0.67 to 0.45) and
acoustic calm matching (0.66 to 0.34). **Only album/genre support adopts the
measured wording in engine v7.** First-pass matching, acoustic/catalog support,
conflict judgments and thresholds retain their existing contracts. Assertions
bind the production metadata question exactly to the measured variant. The
experiment tests selected judgments, not full candidate competition or certification.

The separately authorized full v7/v29 validation completed fresh conformance and
all 82 scenario executions. It used **289 requests**, all with responses, and
13,905,901 reserved input units within the 818-request/19,746,130-unit plan.
Provider usage was **3,258,585 input** and **216,690 output** tokens, with no
retries, uncertain requests or missing usage.

| Result | Supplied v6/v28 | Measured v7/v29 |
|---|---:|---:|
| Passed scenarios | 45/66 (68.2%) | 55/66 (83.3%) |
| Required assignments returned | 137/159 | 148/159 |
| Safety, including repeats | 16/16 | 16/16 |
| Default vocabulary | 40/60 | 49/60 |
| Custom vocabulary | 4/5 | 5/5 |
| Maximum vocabulary | 1/1 | 1/1 |
| Context-only | 6/9 | 6/9 |
| Full quality gate | Failed | Failed |

Ten scenarios improved and none that previously passed regressed. This comparison
includes four corrected inputs, so it is not a pure engine comparison; their old
outcomes remain failed in the historical report. The targeted controlled experiment
above separately establishes the support wording effect.

All eleven remaining missing assignments fail initial matching; none fails
grounding or candidate capacity. Required-tag recall is not the scenario pass rate.
Remaining first-pass scores are Arctic 0.66, city 0.22, shopping 0.55, festival 0.64,
storytelling 0.41, festive dance 0.67, temple 0.66, Renaissance festive 0.43,
acoustic calm 0.65/0.66 and acoustic urgent 0.31. The custom over-tag is resolved.
Next investigate the semantic first-pass question and how the complete definition
is applied to short descriptions. Do not automatically reuse the failed broader
rewrite. Three acoustic positives still require independent listening qualification;
numeric proxies must not be rewritten into guaranteed emotion labels to pass.

Engineering validation passes: 580 Rust tests with no skips, using real FFmpeg and
the pinned optional voice artifact; four developer-example tests; formatting,
architecture, workspace check, strict workspace/fuzz Clippy, doc tests and generated
contracts. No frontend behavior changed. These checks do not certify Jev quality.

Local evidence: `target/jev-support-results-20260928.jsonl` and
`target/jev-quality-v7-20260928/result.json`. Credentials and generated run artifacts
remain outside source control. This repair neither analyzes library audio nor
updates application certification or accepted tags.

## Jev controlled comparison and full validation: 2026-09-27

Pinned `jev-1.13.0` was tested with explicitly authorized synthetic inputs only.
The supplied completed v5/v28 export contains **15/66** passing scenarios (not
16); all 269 requests received responses. Its 100 missing required assignments
comprise 94 initial match rejections, three period applicability rejections and
three grounding rejections. This was a semantic regression, not a transport failure.

A 30-request comparison held six cases and two questions per case fixed while
changing names, question wording and metadata-card framing separately. It reported
35,259 input and 1,245 output tokens. Literal semantic questions plus neutral
descriptive cards improved explicit descriptor matching; names alone did not.
Engine v6 adopts that measured combination. The [ADR](ADR-026-native-jev-evidence-judgments.md#controlled-framing-repair-engine-v6)
records the per-variant observations and their limits.

Fresh conformance and the **unchanged** v28 suite then completed, including all
66 scenarios and 16 safety repeats. This used the production native planner,
executor, durable usage ledger and shared scorer in a new isolated SQLite database.
The run made 289 requests, all with responses, using 13,892,745 conservative
reserved input units. Jev reported 3,258,025 input and 216,283 output tokens.
There were no retries, uncertain requests, missing usage or private-library inputs.
The authorized caps were 818 requests and 20,000,000 conservative input units.

| Result | Supplied v5 | Measured v6 |
|---|---:|---:|
| Passed scenarios | 15/66 (22.7%) | 44/66 (66.7%) |
| Safety checks, including repeats | 16/16 | 16/16 |
| Required tag assignments returned | 59/159 | 135/159 |
| Required mood assignments returned | 6/70 | 60/70 |
| Custom-vocabulary scenarios | 4/5 | 4/5 |
| Context-only scenarios | 6/9 | 6/9 |
| Full quality gate | Failed | Failed |

Thirty scenarios improved and one regressed: the redefined custom `dark` case
correctly returns that label but also returns `quiet focus`, exceeding its one-tag
limit. Custom vocabulary has four passes in both runs, but the failing case changed.
The 24 remaining missing assignments are 15 initial-match and nine grounding
rejections; none is a period-follow-up or candidate-limit rejection.

The remaining work has distinct causes:

- Four positive setting expectations depend on `Origin`, which the current contract
  treats as provenance: tavern in `metadata-prompt-injection`, ruins in
  `melancholy-ruins-expedition`, village in `humorous-village-fair`, and temple in
  `modern-temple-service`. Audit the fixtures and supply explicit descriptive
  evidence in the positive cases, retaining separate provenance-only negatives.
  Do not restore identity-to-setting guesses. These cases remain failed in this report.
- Nine required assignments pass matching but fail grounding. For example, `calm`
  in “Calm Wilderness Travel” matches at 0.82, but its two support scores are 0.62
  and 0.67. The grounding question still mixes observation meaning with recording
  context. A further controlled comparison is needed before claiming a repair.
- Eight other initial-match misses concern use or mood semantics, including
  `city` at 0.21 despite “Royal City Secrets and Conspiracies.” Audit definition
  specificity and complete custom requirements alongside grounding; do not lower
  thresholds or accept mere compatibility to improve recall.
- The three audio-only misses remain both settled-texture variants (`calm`, 0.66)
  and sustained drive (`urgent`, 0.31). The volatile-development positive passes.
  These are handcrafted proxy fixtures, not independently labelled recordings.
  Qualify their expected moods through the listening pilot and compare useful
  upstream features before treating them as evidence of acoustic accuracy.

The 90% quality thresholds, all safety rules, vocabulary cases and nine-case
context gate were unchanged. No application certification was written. The results
establish a substantial harness improvement, not a production-ready Jev model or
listening accuracy. Original records remain locally under
`target/jev-framing-results-20260927.jsonl` and
`target/jev-quality-v6-20260927/result.json`; credentials and generated results are
not tracked. Repeating or extending either paid experiment requires a new reviewed
plan and authorization.

## Library cleanup checkpoint: 2026-09-14

The supplied v1 exports used Thinking disabled. DeepSeek (`deepseek-flash`,
generic OpenAI-compatible adapter) returned all eight responses and passed 6/8.
Only `version` and `reordered` failed without contract errors: valid abstentions
where selection was expected. The old report discarded decisions and reasons,
so the precise rationale is unknown. The prompt lacked a positive selection rule,
and evidence text asserted agreement even when its support flag was false.

Terra (`gpt-5.6-terra`, OpenAI Responses) passed conformance but failed all cases
as `cleanup_model_incomplete`, returning in 193-351 ms without reported usage or
model IDs. A failing regression confirmed that nullable `candidate_id` was absent
from `required`, violating [OpenAI strict-schema requirements](https://developers.openai.com/api/docs/guides/structured-outputs#all-fields-must-be-required).
Cleanup also masked every provider failure as incomplete output. Schema rejection
fits this export; the discarded original error prevents recovering its exact code.

The fix requires explicit nullable candidate output, closes supplied references,
clarifies evidence and selection, and preserves safe errors and validated synthetic
decisions. Suite `closed-catalog-adjudication-v2` retains all eight expected outcomes
and the all-pass gate. The schema and error regressions failed before the fix and
passed afterward. No paid provider run or deployment was performed. Fresh
conformance and complete v2 runs remain required before model certification.

## Operator checkpoint: 2026-09-10

The supplied Sol (`gpt-5.6-sol`, Thinking disabled) v23 export contains a full run
at 19:39 UTC and a later diagnostic retest at 19:43 UTC. Conformance passed. The
full run completed all nine provider requests and reported zero reasoning tokens;
there is no request, schema or output-limit failure in this evidence. It passed
59/63 scenarios and all 13 safety checks. Bundled vocabulary scored 53/57, custom
5/5 and maximum 1/1, but the independent context-only gate failed at 8/9.

The four omissions were `heroic-castle` (heroic), `metadata-prompt-injection`
(tavern), `arctic-escape` (cold), and `acoustic-context-sustained-drive` (urgent).
The retest recovered heroic and urgent, producing a merged diagnostic score of
61/63 and context-only 9/9. It reran only those four cases and the injection
case's safety repeat; it is not a second full-suite pass. Certification correctly
remains failed. The tavern omission persisted despite supplied origin `Old River
Inn`; the model continued to ignore the injected instruction safely. These runs
show output variability, not a basis for preferring Sol over other models.

Source inspection also found a fixture defect: sustained-drive intensity was
0.83 although the supplied opening/ending measurements imply
`0.5 * 0.5 + 0.3 * 0.89 + 0.2 * 0.8 = 0.677` under the actual DSP formula.
Suite v24 corrects the intensity trajectory and section value; a regression
checks that steady acoustic controls preserve this relationship. No expected
tag, confidence rule, prompt, model setting or quality threshold was changed.
The export does not show that this inconsistency caused Sol's abstention; the
same old fixture passed its retest. Numeric coherence is separate from whether
synthetic expected moods match independent listening judgments.

Keep the chosen model/Thinking while obtaining fresh conformance and one complete
v24 check after installation. Failed-case retests remain diagnostic; do not repeat
subsets until a merged score is treated as certification. Follow a full pass with
the existing reviewed listening pilot before claiming useful mood accuracy.
No paid provider run or deployment was performed for this fixture correction.

Local validation: the new consistency regression failed against v23 and passed
after correction. All 424 Rust tests passed, along with formatting, workspace
check, strict Clippy, architecture checks, doc-test command and generated-contract
verification. A structural diff confirmed that only one fixture's intensity and
the suite ID changed; all 63 scenarios' expectations and thresholds are intact.

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
