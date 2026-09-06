# Mood context: quality review and next evaluation

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
tagging. A model that passes tagging still needs to pass cleanup independently.
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
| 2. Verify the provider boundary | Tests cover the request/parser and durable lifecycle; address any real adapter failure. | Save Music tagging to link the shared model; run both strict task checks. Try at most 20 tracks with explicit request/reservation limits after local context refresh. Check provider billing and collected review-only results. | Real Batch completes or clearly reports its error; no unexplained repeat requests. |
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
