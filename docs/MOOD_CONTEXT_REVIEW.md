# Mood workflow: implemented rework and remaining evaluation

## Interface update — 2026-09-11

Mood-tag cleanup, including local suggestions and optional legacy AI maintenance,
has been removed from AI setup, the test console and Mood vocabulary. Manual tag
editing and rename/merge remain. Existing server contracts and stored records are
unchanged. Specialized audio analysis now has a selectable **Planned** test-console
entry; it cannot configure an analyzer, run tests or upload audio yet. This supersedes
the cleanup-interface description in the September 8 implementation below.

## Implementation — 2026-09-08

The application rework is implemented. Musical accuracy and adoption of a new
audio classifier remain unproven, and no paid provider or private-library run was
performed. The design assessment below records the pre-change diagnosis.

- **One normal tagging setup.** Cleanup is collapsed under optional legacy
  tag-name maintenance and excluded from normal tagging readiness. Deterministic
  vocabulary maintenance and historical cleanup records remain available.
- **Explain every saved outcome.** Review exposes tag count, public explanation,
  confidence, recorded context coverage and, for new profiles, the exact bounded
  per-track input. Empty results stay visible; older missing explanations/inputs
  are reported honestly. Filters separate returned tags from no tags. Selected
  reconsideration follows normal planning/consent and never rebuilds unselected
  tracks by default. Manual/accepted tags remain untouched.
- **Bound waste.** Default pilots are 20 tracks. The enabled no-tag guard saves a
  valid empty request and stops before another request. Ordinary continuation
  skips current empty profiles. Multi-request asynchronous plans require a
  deliberate guard override. Corrected counters distinguish current/deferred,
  processed/with-tags/empty, saved/changed and remaining work. Returned track
  outcomes and partial counters survive in job records and exports.
- **Improve the task.** Mood impressions and session-use suggestions are labeled
  separately. Broad acoustic impressions may be proposed with restrained
  confidence; nuanced emotion, setting, scene and period still need semantic
  support. No minimum tag count, title inference or local mood guessing was added.
  Every future abstention requires an explanation. Balanced structural examples
  replace the repeated empty example.
- **Test the sparse-metadata use case.** The suite grows from 56 to 63 scenarios,
  with 13 safety repeats and an independent context-only gate. Seven new cases
  cover calm/urgent/chaotic context, gain changes, conflicting endings, weak tempo
  and unavailable measurements. Metadata success cannot mask failure on those
  cases; existing vocabulary and safety gates remain.
- **Compact the input.** Shared live/evaluation projection rounds numeric detail
  and removes sampled tempo points and repeated prose while retaining all ten
  bounded sections, endings, development, voice and reliability. In the fixed
  synthetic 20-track regression, user payload falls from 102,461 to 70,261 UTF-8
  bytes (31.4%); the new system prompt is 11,304 bytes. This isolates projection
  savings using the same current vocabulary/envelope, not a reconstruction of the
  operator's exact request or a billed-token comparison. The proposed 50% overall
  target is not demonstrated.

Contracts advance to input v21, output v4, disclosure v13, analyzer v7 and suite
baseline v22. Existing `local-context/v2` results are reusable; no algorithmic or
voice rerun is required by this change. Earlier AI profiles become outdated and
the tagging task needs current conformance/quality checks before new inference.
Older saved results remain inspectable. Nothing starts automatically.

**Still to do, in order:** the operator reviews one fixed 30-track listening
sample; engineering compares retained/compact results and profiles one compatible
music classifier if the evidence justifies it; integrate/cache a useful winner;
only then authorize scaling. The offline cohort/scoring tool and exact first
candidate compatibility findings are in [the listening pilot](MOOD_PILOT.md).
The candidate needs a separate Discogs-EffNet encoder and cannot consume existing
voice scores. No new model weights, runtime service or dependency were added.

Engineering validation covers mocked request/cancellation/continuation, Batch
recovery, strict schemas and profile provenance, UI state and offline scoring.
The explanation panel and controls were also inspected in a real local browser
using synthetic data. Private listening accuracy and provider-reported savings
remain the operator-assisted acceptance boundary.

Validation: 388/388 Rust nextest cases passed; workspace check, strict Clippy,
formatting, doc tests, architecture and generated-contract checks passed. The
frontend suite passed 301 tests, followed by all five dialog tests after adding
the selected-reconsideration regression (302 distinct current tests covered);
lint, typecheck and production build passed. The four offline pilot tests pass
and are included in CI. Windows FFmpeg tests required permitted child-process
access; no test or guard was skipped to obtain the pass.

## Pre-rework design assessment — 2026-09-08

**Historical diagnosis and implementation proposal.** See the implementation
status above for what has now changed.
Reviewed source at `a66b039`, the supplied screenshots and the operator's report
that algorithmic analysis completed for the library. No private database, exact
production request/response, or audio was inspected. No paid model call was made.
This assessment supersedes the forward-looking September 6 plan below; its old
implementation measurements remain historical evidence.

Mood-tag cleanup is not required for tagging. The previous change unified model
settings but retained a separate cleanup task and certification. The setup copy
and the previous instruction to run both checks made this look like a required
pipeline. That guidance was wrong for someone who only wants tagging.

The central problem is a mismatch between the available evidence and the requested
result. A text model receives metadata and acoustic measurements, not music.
Completing every local stage improves coverage; it does not turn DSP and voice
detection into a musical mood, instrument or scene recognizer. The prompt requires
explicit semantic evidence for setting, scene and period tags. Sparse metadata can
therefore produce a valid but useless abstention even with full audio context.

Keep local analysis, bounded execution and human review. Simplify setup and develop
audio-based musical evidence before scaling paid tagging. More thinking, retries
or a second cleanup model would not address missing evidence.

### Findings

| Finding | Source evidence | Consequence |
| --- | --- | --- |
| Cleanup is independent | `providers.rs::ensure_shared_mood_configuration` checks shared settings only for `tag_cleanup`. `ModelTaggerBatch::finish` accepts supplied canonical IDs only. | Generated names are already canonical. AI cleanup concerns ambiguous operator-owned names, not a second pass over model tags. |
| Empty results are valid and cached | The schema permits zero tags; `model_tag_profile_is_current` accepts valid empty profiles. Jobs skip current profiles regardless of tag count. | A completed job can save no suggestions. Repeating an unchanged run skips them; forcing a rebuild can spend again without better evidence. |
| Abstention evidence is hidden | `tags.rs::view_for_track_with_model` exposes evidence only inside the loop over tags. `ModelAnalysisStatus` omits evidence and the stored `context_status`. | An empty profile's explanation disappears. Evidence can also be empty, so some older results may have no reason to recover. |
| The inspector displays different evidence | `LibraryTagEditor.tsx` shows `audio_signal` from `local-audio/v1`; the tagger uses bounded `local-context/v2`. | The displayed measurements do not show exactly what the AI used. This does not prove the production request omitted full context. |
| Completion does not measure usefulness | `model_jobs/tagging.rs` counts stored profiles, including empty ones, without a tag-yield stop. | A run can declare suggestions ready while every result is empty and continue spending on further requests. |
| Deferred tracks can be called current | `unchanged_profiles` subtracts the truncated work list from eligible tracks. | Tracks deferred by the run limit are included in “already current.” Count current and deferred work before truncation. |
| Quality fixtures miss the real use case | Only 5 of 56 cases in `music-tagging-v1.json` contain context. Three also contain explicit semantic metadata; two are restraint cases. None includes production `measurement_reliability`. | No positive case requires useful mood inference from acoustic context alone. A 56/56 pass establishes synthetic contract/semantic performance, not musical usefulness on this library. |
| Most tags need unavailable semantics | The default vocabulary has 49 settings, 8 periods, 42 scenes and 39 moods: 138 total. | Most choices cannot be supported by generic acoustic measurements under the current prompt. |
| The example only demonstrates abstention | `tagging_example` repeats empty tags and insufficient-metadata evidence for every slot. The harness says it teaches structure only. | Possible additional abstention bias; this is a hypothesis, not proven model reasoning. |

The reported full-analysis run rules out incomplete analysis as the working
explanation. Current profiles and zero suggestions are consistent with saved
empty tag sets. Inspection found no normal parser path that silently discards all
valid canonical tags. Exact abstention reasons and historical context still need
the retained records. Expose those first, without paying to recreate the failure.

Source entry points: [tagger](../crates/music-application/src/assistant/model_tagger.rs),
[job](../crates/music-application/src/assistant/model_jobs/tagging.rs),
[review projection](../crates/music-application/src/assistant/tags.rs),
[roles](../crates/music-application/src/assistant/providers.rs),
[fixtures](../crates/music-application/src/assistant/evaluation_suites/music-tagging-v1.json),
[review UI](../frontend/src/views/assistant/AnalysisTagReview.tsx),
[editor](../frontend/src/views/assistant/LibraryTagEditor.tsx).

### Cost and usefulness

The operator reported 3 responses for a 50-track pilot, with 119,136 input tokens,
2,074 output tokens and no visible suggestions: about **2,383 input tokens per
track** with zero demonstrated yield. Cache reads were 11,092 tokens, about 9.3%
of input. Cache reads/writes are already included in totals; do not add them again.
These are reported usage figures, not a verified bill or portable price estimate.

Reconstructing the current default vocabulary projection gives **32,225 UTF-8
bytes per request**, before track evidence. This is not a token count. Definitions,
aliases and cues accompany trajectories, sections and prose describing overlapping
facts. Aggregate usage cannot identify each component's exact production cost.
The older synthetic request-size measurements below are not this private run.

Batch can change scheduling and price; it does not improve an uninformative
classification. Measure useful accepted tags, missed useful tags and false positives
alongside cost. More tags or more cache hits alone do not establish success.

### Proposed workflow and architecture

Keep the existing library browser and directly editable manual tags. Use three
clear steps: **Prepare evidence → Suggest tags → Review results**. Preparation
distinguishes analysis coverage from musical-classifier availability. Suggestions
use one optional text-model configuration and explicit scope/budgets. Review filters
separate tracks with tags, abstentions, failures and outdated results; every result
shows what happened, why, and the evidence used.

Retain execution outcome, result content and freshness as separate fields. A
processed track may have a current abstention or an outdated positive result.
Summaries need tracks with tags, total tags, abstentions, errors, current profiles,
skipped changes and deferred work. Preserve bounded run membership/outcomes apart
from the replaceable latest-profile cache. Old profiles expose whatever evidence
exists, otherwise “reason not recorded.” Future abstentions require a concise
public reason, not hidden reasoning. Store a bounded authenticated input/result
snapshot without credentials or audio; do not present reconstructed current
context as the exact historical input.

Separate three kinds of evidence and suggestions:

| Layer | Meaning | Source |
| --- | --- | --- |
| Acoustic facts | Approximate pulse, voice presence, relative development and significant section changes | Existing local analysis; useful for browsing without paid inference. Recording level stays separate from emotional intensity. |
| Musical impressions | Relaxed, melancholic, tense; instrumentation/style when supported | A music-specific audio model and bounded metadata. Predictions retain uncertainty and are not automatic manual tags. |
| Suggested session uses | Could suit rest, pursuit, exploration or a tavern | Editorial suitability based on musical evidence and operator vocabulary meanings, with separate provenance and explicit review. |

This resolves asking what a song *could suit* while demanding metadata that
literally names that situation. It does not justify loudness-to-combat or
slow-tempo-to-rest rules. Unknown periods remain unknown; never force minimum tags.

Target: **local audio → cached musical evidence → optional text interpretation →
canonical-ID validation → human review**. No AI cleanup stage. Keep deterministic
alias, spelling and duplicate review in Vocabulary maintenance. Retire standalone
AI cleanup from normal setup while preserving authored tags and historical records.
Any later ambiguous-name AI helper needs its own justification and safeguards.

The voice runtime currently selects exactly two predictions, voice/instrumental.
Its published graph exposes intermediate features, but adding moods requires
compatible trained heads or another checkpoint, not enabling a dormant setting.
Reuse decoding/windowing where compatible and assess a shared encoder with small
classifier heads before adding multiple full passes.
[Voice model metadata](https://essentia.upf.edu/models/classifiers/voice_instrumental/voice_instrumental-musicnn-msd-2.json).

Limit model exploration to two candidates:

- **First: established music classifiers.** Essentia publishes mood/theme models,
  including an MTG-Jamendo taxonomy, and demonstrates embedding reuse with downstream
  classifiers. These can supply musical evidence, but do not certify tabletop tags.
  MTG lists CC BY-NC-SA 4.0 or proprietary licensing for its models. Check the exact
  artifact, Rust compatibility and resource cost before adoption.
  [Catalog](https://essentia.upf.edu/models.html),
  [embedding workflow](https://essentia.upf.edu/tutorial_tensorflow_auto-tagging_classification_embeddings.html).
- **Alternative: music-trained audio/text matching.** LAION's `larger_clap_music`
  card describes audio/text similarity and zero-shot classification and labels that
  artifact Apache-2.0. Compare audio with musical descriptions attached to vocabulary
  entries, including negative/abstention examples. Similarity is not a probability;
  always choosing the closest label would recreate false tags. Resource use and
  runtime compatibility remain unverified.
  [Model card](https://huggingface.co/laion/larger_clap_music/blob/main/README.md).

These are research candidates, not accepted dependencies or accuracy promises.
Preserve the Rust runtime and analysis port. Do not introduce a Python production
service, send songs to a provider or restore filename/title inference for this work.

### Finite implementation plan

| Order | Engineering work | Completion criterion |
| --- | --- | --- |
| 1. Explain and simplify | Expose profile evidence/confidence/context status; distinguish empty/failed/stale; fix current/deferred counts; remove cleanup from tagging setup and correct help. Allow selected-track reconsideration instead of directing users to rebuild everything. | Existing empty results become diagnosable without another provider request. Tagging readiness never requires cleanup certification. Empty profiles are not described as generated tags. |
| 2. Stop avoidable waste | Persist tag-yield counts. Default new standard pilots to stop before scheduling another request when the first completed request yields no tags; preserve results and require deliberate continuation. Submit asynchronous Batch pilots separately: already submitted work cannot be unspent. | Tests cover zero-yield stop, continuation without duplicate paid work, cancellation and partial counters. Valid abstention does not trigger corrective retries. |
| 3. Rework task and evaluation | Separate mood from session suitability; add positive context-only cases, sparse metadata, mixed sections, contradictory cues and production reliability/missingness. Use balanced synthetic examples instead of all-empty examples. | Always-abstain fails positive usefulness tests. Existing safety checks and quality thresholds remain. Version changed prompts/schemas/disclosures/fingerprints as applicable. |
| 4. Compare compact context and one music model | Compare existing context, compact evidence and musical classifier evidence on one fixed cohort, retaining the same text model/Thinking setting. Start with the first candidate; investigate the alternative only for a demonstrated gap. | Select or reject using useful-tag precision/coverage, per-group errors, provider usage and local CPU/RSS. No speculative dependency expansion. |
| 5. Integrate the winner and scale | Cache audio evidence by source/model/preprocessing identity and vocabulary interpretation separately. Integrate the useful source into existing review contracts; retain optional bounded text/Batch processing. | Unchanged runs do no new inference. Vocabulary edits do not require decoding unchanged audio. A reviewed pilot passes before a larger authorized run. |

Compact input retains trends, significant transitions/endings, voice coverage and
uncertainty; removes repeated prose and false precision. Send all meanings in the
explicitly requested tag groups. Do not silently prune custom vocabulary using
untested heuristics. Target 50% less input, conditional on preserving useful coverage;
measure actual reported tokens as well as bytes.

Use 30 varied tracks: 20 for development and 10 held out, spanning artists, styles,
dynamics, voice states and metadata richness. Avoid an alphabetical single-artist
slice as the only sample. The operator identifies useful tags and unacceptable
suggestions independently of model output. Keep private examples local.

Proposed pilot targets, to agree before comparing alternatives: at least 80% of
proposed tags judged useful, and a useful suggestion for 60% of tracks where the
operator identified a supported target tag. Score moods and session uses separately;
retain all existing safety gates. Report raw counts: this small pilot is a go/no-go
sample, not a general accuracy claim. Include certification, corrections and live
comparison in one explicit usage budget; no automatic paid retesting/model shopping.

### Ownership and stopping point

**Engineering:** diagnostics, API/UI changes, regressions, compact input, model
compatibility/resource checks, local benchmark tools and comparison reports. Steps
1–2 require no paid library run. No unrelated playlist/EQ expansion is needed.

**Operator:** make the existing records accessible through the new inspector,
review one small listening sample, decide which suggested session uses are helpful,
and authorize a capped provider comparison when ready. Deployment remains in the
infrastructure repository. No complete local-analysis rerun or cleanup certification
is needed merely to diagnose this run.

If the tested approach misses the pilot targets, retain useful local filters and
manual tags and leave paid tagging experimental. Stop spending or adding models
until there is a concrete new reason to expect improvement.

**Review validation:** source tracing, programmatic fixture/vocabulary counts and
documentation checks. Production usage was supplied by the operator. No runtime
change, private musical benchmark or new accuracy claim is made in this review.

## Historical implementation review — 2026-09-06

**Reviewed:** 2026-09-06. Scope: current Rust analysis, model input preparation,
cost controls and music-tag usefulness. No private-library or paid-provider run.

## Assessment

The architecture is worth keeping: local audio analysis, a bounded optional model,
and explicit human review have distinct jobs. The main remaining uncertainty is
musical usefulness, not JSON generation. Better batching reduces cost; it cannot
make an acoustic statistic prove a scene such as a medieval tavern.

One shared text model is sufficient as an architectural default. Tagging chooses
canonical IDs; cleanup resolves ambiguous *manual tag names*. Local rules already
resolve canonical names and declared aliases, and cleanup is not chained after
tagging. Only someone using AI cleanup needs to pass its independent check;
tagging alone does not require cleanup configuration or certification.
The supplied Luna cleanup export failed one of twenty strict cases, so unification
does not establish that Luna is good enough for both tasks.

## Confirmed defects fixed in this change

### Spectral analysis missed most of each frame

Previously one centered 2,048-sample FFT represented each 8,000-sample half-second
frame at 16 kHz: about 25.6% temporal coverage. RMS and onset analysis covered more
audio, but spectral measurements could miss transients outside that short window.

Controlled 40-second synthetic WAVs contained a continuous 200 Hz tone plus the
same 80 ms, 4 kHz burst every half second. Moving the burst from 210 ms to 10 ms
changed typical brightness from **0.9190 to 0.0000**, despite unchanged level and
spectral content over the full interval. Overlapping FFT windows now cover the
whole frame, giving **0.9161 and 0.9052** respectively. A regression checks five
burst positions; the single-FFT two-tone calibration is retained independently.

This is `local-context/v2+rustfft/v2`. Old local context must be recomputed; model
runs do not consume stale context and are never started automatically. The probe
uses generated tones, not representative music, and is not a throughput benchmark.

### Model projection omitted reliability and the ending

The analyzer permits ten sections, but the model received only the first eight.
It also received overall confidence while per-measurement reliability was dropped.
The projection now retains all ten bounded sections and measurement reliability.
Task instructions explain that coverage confidence, onset activity, spectral spread
and voice scores are limited evidence. Titles and filesystem paths remain excluded.

## Remaining design concerns

| Finding | Evidence and consequence | Recommended change |
| --- | --- | --- |
| Intensity depends strongly on recording gain | Intensity is 50% normalized RMS level, 30% onset activity and 20% spectral density. Attenuating the same synthetic signal by 20 dB reduced typical intensity from 0.3761 to 0.1272 before the spectral fix; the effect remains afterward (0.3777 to 0.1290). A quiet master can appear calmer. | Separate absolute recording loudness from musical activity. Compare gain-normalized features and relative within-track dynamics before choosing a revised intensity mapping. Do not normalize or rewrite library audio. |
| Tempo is quantized and may be half/double time | The 20 Hz onset envelope uses integer autocorrelation lags. Synthetic 120 BPM returned 120; 130 returned 133.33, with a reported confidence of 0.626. Typical values are points on a coarse grid, not exact tempos. | Benchmark a finer onset envelope/interpolated estimator against a beat-tracking reference; test silence, sparse ambience, swing, half/double time and changing tempo. Keep unresolved/approximate states. |
| Global confidence describes coverage | “High” primarily requires at least 30 seconds and sufficient active audio. Most axis reliability labels are fixed medium. Neither is calibrated against mood accuracy. | Name coverage explicitly in the UI and develop measurement-specific reliability from controlled tests. Do not use the global label as a reason to accept a semantic tag. |
| Density and drive are proxies | Density combines entropy, occupied bands, bandwidth and flatness. Drive mostly reflects onset activity. Neither establishes instrument count, danceability or urgency. | Keep factual descriptions in model input. Only add semantic labels after validating them on reviewed music. |
| Metadata remains indirect evidence | Artist/album/origin/genre may help, but sparse metadata plus simple DSP does not establish instrumentation, setting or emotional intent. | Prefer bounded, operator-reviewed descriptions and reliable genre/instrument evidence. Preserve source provenance and missingness. Do not restore misleading filenames as tagging evidence. |
| Context has redundant detail | Trajectories, sections and prose evidence repeat related facts; five-decimal values can imply more precision than the heuristics justify. | Compare a compact typed summary using useful ranges, direction, peak location and reliability. Retain salient endings and transitions. Choose it by quality/cost ablation, not byte count alone. |

The existing voice classifier is a useful optional acoustic source, not a mood
model. Its published metadata describes voice/instrumental classification at 16 kHz
and reports 0.98 normalized cross-validation accuracy on 1,000 in-house excerpts.
That is not calibrated probability or accuracy on this library. The current rolling
analysis covers the track and reports both score and vocal-window coverage; preserve
explicit unavailable/unknown states. [Model metadata](https://essentia.upf.edu/models/classifiers/voice_instrumental/voice_instrumental-musicnn-msd-2.json).

## Research implications

EBU R 128 separates programme loudness, loudness range and true peak. This supports
keeping level as a technical descriptor rather than treating it as musical emotion;
it does not specify a correct intensity formula for this application. The analyzer
already computes loudness information locally, so first evaluate that existing data.
[EBU R 128](https://tech.ebu.ch/publications/r128).

Essentia's rhythm extractor returns beats, tempo distribution and confidence with
explicit method limitations. It is a useful offline comparison reference; replacing
the Rust runtime with another stack is unnecessary. Keep the current application
port and benchmark any replacement before adopting it.
[RhythmExtractor2013](https://essentia.upf.edu/reference/std_RhythmExtractor2013.html).

Richer local music encoders are a plausible next experiment, not an immediate
dependency. MERT learns acoustic music representations; CLAP connects audio and
text for retrieval/classification. Neither establishes reliable custom RPG scene
tags without library-specific evaluation. If basic context is inadequate, compare
one cached local embedding pass plus deterministic vocabulary retrieval against
the text-model baseline. Only then consider adding a text model for ambiguous cases.
Check deployment resources, model terms and output provenance for the chosen artifact.
[MERT](https://arxiv.org/abs/2306.00107), [CLAP](https://arxiv.org/abs/2211.06687).

## Efficiency measurements and limits

Exact synthetic production request-builder measurements, in UTF-8 bytes:

| Request | Before | After | Interpretation |
| --- | ---: | ---: | --- |
| 20 tracks, metadata only | 55,348 | 56,232 | The complete vocabulary and clearer instructions are retained. |
| 20 tracks, rich fixture context | 109,448 | 110,332 | No claim that raw payloads became smaller. |
| Cleanup of 20 unresolved names | 35,449 | 35,450 | Still a separate optional request. |

The 32,225-byte vocabulary is now in a stable user-message prefix. Changing only
local database IDs no longer changes the system prompt or output schema. This makes
repeated equal-sized requests eligible for useful prefix reuse. Actual cache hits
depend on provider/model/timing and must be read from returned usage.
Cache hits also do not remove OpenAI input tokens from tokens-per-minute limits.
A financial budget, a rate limit and a daily/provider allowance are different constraints;
without the earlier run's usage and provider billing record, its exact exhaustion cause
cannot be established. Batch has separate scheduling/rate capacity, but still costs money.
All 138 canonical vocabulary names require **zero model cleanup batches** in the
synthetic local-cleanup probe.

The largest predictable savings are skipping unchanged results, avoiding a mandatory
cleanup model pass, bounding each run and using supported asynchronous Batch. OpenAI
documents a 50% Batch discount and completion within 24 hours. Do not assume this
stacks with a particular cache discount or predicts this account's bill.
[Batch guide](https://developers.openai.com/api/docs/guides/batch),
[cache guide](https://developers.openai.com/api/docs/guides/prompt-caching).

## Plan and ownership

| Step | Engineering work | Operator work | Exit condition |
| --- | --- | --- | --- |
| 1. Ship the bounded workflow | Implemented: limits, shared configuration, Batch recovery, accounting, stable identity, spectral coverage and reliability projection. Validate repository gates. | Deploy through the normal infrastructure workflow; retain the database backup and credential key. | Candidate starts, schema 12 is current, accepted/manual tags survive. |
| 2. Verify the provider boundary | Tests cover the request/parser and durable lifecycle; address any real adapter failure. | Verify and certify only the task being used; tagging does not require cleanup certification. Try at most 20 tracks with explicit request/reservation limits after local context refresh. Check provider billing and collected review-only results. | Real Batch completes or clearly reports its error; no unexplained repeat requests. |
| 3. Establish musical quality | Prepare a fixed, small comparison and aggregate errors/cost per useful accepted tag. Test improved intensity/tempo/context representations locally. | Select 20–50 varied tracks, listen, mark useful and unsupported tags, and allow abstention. | A reviewed baseline distinguishes semantic quality from schema success. |
| 4. Improve only demonstrated weaknesses | Compare metadata-only, metadata plus current context, and a compact calibrated context, using the same model/Thinking and vocabulary. Consider one local encoder only if needed. | Choose the balance between missing useful tags and false scene/mood tags. | Quality improves at a measured, bounded cost; then expand gradually. |

Keep EQ expansion and new audio-model integrations deferred. Do not start another
whole-library paid run merely because conformance passes. A small pilot is now
bounded and reviewable; a large run should follow evidence that the tags help.

## Implementation validation

- Full Rust suite: 384 tests passed. The final prompt-wording/cache-family adjustment
  additionally passed the 22 relevant regressions.
- Frontend: 290 tests in 51 files, strict lint, type checks and production build passed.
- Rust strict Clippy, fuzz-target Clippy/formatting, doctests, architecture checks,
  generated HTTP/protocol contract checks and release headless build passed.
- Workspace and fuzz dependency/license/advisory checks passed against the refreshed
  RustSec database; no unused dependencies were found.
- No paid configured-provider calls, deployment, Docker release-image check or
  private-library throughput/quality run was performed. Those remain operator acceptance.
