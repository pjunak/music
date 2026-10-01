# Targeted Jev audio evidence, 1 October 2026

Status: complete; 144 provider responses exported and compared. This is developer
research; it does not change production tagging or certify listening accuracy.

## Question and experiment sequence

The [previous feature study](JEV_ALGORITHMIC_FEATURE_PILOT.md) found no clear
benefit from a large numerical stack. It also found substantial sensitivity to
the presentation of measurements. This follow-up tests smaller musical inputs
and separates numerical, temporal and explanatory information.

1. Bind the same 13 originals to the frozen labels8 baseline by SHA-256 and
   duration. Keep titles, paths, owner descriptions and web descriptions local.
2. Extract beat-period candidates and ambiguity, repeating accent patterns,
   spectral roughness and pitch-profile development. Check controlled signals,
   gain invariance, missing values and coverage before interpreting them.
3. Reaggregate the existing instrument/style classifier windows into three
   equal-duration bins. Use the same eight labels per head as the baseline;
   compare pooled values with time-local values to separate duplication from
   temporal information. Classifier scores remain uncalibrated and correlated.
4. Freeze ten input arms with the same 138 short Noul questions and pinned
   `jev-1.13.0`. Rotate primary arms, repeat the exact baseline per song, and
   include one empty-evidence control. Review exact payloads and reservations.
5. Run within the owner's continuing explicit authorization to use the temporary
   development key for these experiments. Checkpoint attempts before external
   cost; never retry or replay uncertain requests.
6. Export a complete validated journal and report raw probabilities, usage,
   repeat variation, individual input effects and interactions. Compare partial
   owner-positive descriptions descriptively, without treating unmentioned tags
   as negatives or selecting a new cutoff.

## New information and its limits

| Input | Measurements | Hypothesis | Limitation |
|---|---|---|---|
| Beat and accent | Candidate BPM, onset-period support, competing-peak margin, tracked-beat interval variation, repeating three/four-beat accent fit | Pacing and steady movement may be better represented than note-onset rate | Candidate tempo can be half/double time; fitted beats and accent templates are not verified meter or calibrated confidence. |
| Harmonic texture | Normalized spectral roughness proxy mean/p90, best pitch-template correlation, major-minus-minor template margin, adjacent pitch-profile changes | Distinguish smooth sustained texture, beating partials and tonal development | Sensory roughness is not musical dissonance, emotional tension or danger. Template correlation is not a verified key, mode or mood. |
| Time-local learned evidence | Instrument/style responses for opening, middle and final thirds; pooled counterparts from the same values | Whole-recording means may hide changing instrumentation or texture | Equal thirds are not detected sections; eight-label shortlists omit other classes. Overlapping encoder windows blur boundaries and all heads share the same encoder. |

Primary references: [librosa beat tracking](https://librosa.org/doc/0.11.0/generated/librosa.beat.beat_track.html),
[Sethares' spectral dissonance program](https://sethares.engr.wisc.edu/comprog.html),
[key-template methods and limitations](https://essentia.upf.edu/reference/std_Key.html).
The implementation's normalization, peak selection and aggregation are experimental
adaptations, not claims of equivalence to a calibrated perceptual model.

Physical extraction uses complete 16 kHz mono PCM. Rhythm uses a 2048-sample
STFT with 256-sample hops and a positive, median-adjusted log-mel onset envelope.
Pulse support is nonzero-lag autocorrelation divided by zero-lag energy. The
strongest 30–300 BPM lag explicitly seeds the beat tracker; the competing-peak
margin excludes only its two-lag neighborhood, retaining half/double ambiguity.
Candidate gating requires four onsets, support at least 0.12, and sufficient
onset activity; these are implementation gates, not calibrated confidence.

An early control exposed a false four-phase accent caused by onset-frame
alignment for equal 120 BPM pulses. The corrected measurement uses waveform
RMS in fixed beat-centered ±0.1-second windows, discarding truncated windows.
Equal pulses then score zero for both accent fits; deliberate three-phase
accents score 1.00/0.083 and four-phase accents 0.066/1.00. These controls establish
implementation behavior, not meter recognition on music.

Harmony uses a 4096-sample STFT with 512-sample hops, harmonic chroma and 24
rotated Krumhansl templates. Roughness samples local peaks approximately every
half-second, including the ending: 40–5000 Hz, at most 24 peaks, relative amplitude
floor 0.001. The pair curve uses the publisher's constants, but the bounded
gain-normalized pair weighting differs from its amplitude aggregation. Adjacent
valid one-second pitch profiles yield L1 change; missing seconds are not bridged.
Per-estimate observed coverage is retained locally, separately from decoded
whole-recording duration.

Temporal profiles reuse 2,771 cached, source-bound classifier windows without
another inference pass. Exact rational sample thirds partition each patch's time
ownership, including the ending. Pooling reproduces the unrounded baseline means
within floating-point precision; differences from its rounded responses are
below 0.0005. The supplied temporal responses are rounded to six decimals.

## Fixed comparisons

| Arm | Change from the frozen baseline |
|---|---|
| `baseline` | Exact prior evidence and questions. |
| `beat` | Six compact beat/accent metrics. |
| `harmony` | Five compact roughness/pitch metrics. |
| `physical` | Both physical families. |
| `profile_means` | Pooled instrument/style scores derived from the three bins. |
| `profile_timeline` | The same instrument/style label IDs with three time-local score sets. |
| `all_numeric` | Both physical families plus the time-local profiles. |
| `all_described` | Identical values with short factual metric definitions. |
| `all_no_mood_head` | `all_numeric` with only the baseline mood/theme observation withheld. |
| `missing_all` | Added numerical values explicitly unavailable; original baseline observations retained. |

The missing-value arm retains metric IDs, units, interval fractions and profile
label IDs while withholding measurements, responses and coverage seconds. It therefore
retains some semantic information and is an unavailable-data control, not a
content-free placebo. The definitions arm changes a presentation bundle rather
than isolating a single sentence. Removing the mood head is a separate ablation,
not evidence that the new measurements can replace its semantics.

Ten primary arms and one exact baseline repeat per recording plus one null
control total 144 requests. Hard bounds are 160 requests and six million
conservative input units; actual reservation is frozen in the offline plan.
The seven varied songs remain unrated, and the six earlier descriptions are
partial positives, including tentative and interpretively mapped terms.

Noul values are probabilities of answering yes to the stated tag-fit question,
not a measured percentage of musical intensity. A changed value, a larger tag
list or a more confident answer is not by itself better tagging.

## Tooling and evidence

The native `jev-compare` example's `targeted-plan`, `targeted-run` and
`targeted-report` commands consume `jev-targeted-features/v1`. They reconstruct
and validate the exact original baseline, input identities, complete coverage,
metric ranges, profile labels, request questions, reservations and saved plan.
Unknown input fields and plan drift fail before credential access. Native semantic
JSON fingerprints and source-file byte hashes are separate identities; JavaScript
number serialization must not substitute for the native fingerprint. The runner
uses the existing durable no-retry journal and refuses output overwrite.

```powershell
cargo run --locked -p music-server --example jev-compare -- targeted-plan target/baseline.json target/targeted-features.json target/targeted-plan.json --tracks 1,2
# After reviewing the printed hash and exact request/unit reservation:
cargo run --locked -p music-server --example jev-compare -- targeted-run target/baseline.json target/targeted-features.json target/targeted-plan.json --key-file <private-key-file> --plan-sha256 <printed-hash> --max-requests <printed-count> --max-input-units <printed-units> --output target/targeted-requests.jsonl
cargo run --locked -p music-server --example jev-compare -- targeted-report target/baseline.json target/targeted-features.json target/targeted-plan.json target/targeted-requests.jsonl target/targeted-result.json
```

Private extraction scripts, raw windows, measurements, plans, journals and song
reports are retained under ignored `target/jev-targeted-research-20261001/`.
Only anonymous, reconstructed evidence reaches Jev; originals and library tags
are unchanged. No production dependency or runtime extractor is added.

## Results

All 144 attempts returned complete typed responses, with no retries and no missing
usage reports: 1,195,272 input and 378,864 output tokens. At the
[published rate](https://docs.typesafe.ai/models) of $0.042 per million input
tokens and free output, the input-cost estimate is **$0.050201**; this is not an
invoice. The frozen reservation was 4,438,382 conservative input units. The native
plan identity was `86cb14c854b3aec764ad5d758805524603b3dad696a419e5b356cd063f4c50db`.

| Variant | Mean input tokens/song | Mean change on 22 partial owner-positive descriptions, pp |
|---|---:|---:|
| Baseline | 7,314 | 0.00 |
| Beat/accent | 7,616 | -1.73 |
| Roughness/pitch | 7,577 | +1.18 |
| Both physical families | 7,869 | -1.23 |
| Pooled instrument/style | 7,955 | -0.14 |
| Three-part instrument/style | 8,793 | +0.82 |
| All numeric additions | 9,349 | -0.23 |
| All additions with short definitions | 9,600 | +1.00 |
| All numeric, mood/theme withheld | 9,106 | +0.41 |
| Unavailable added values | 8,954 | -2.23 |

These are signed probability changes, **not accuracy or intensity scores**. The
22 descriptions include uncertain, synonym-mapped and imagery terms; the ten
explicit positives show changes of +0.50 pp for harmony, +0.70 pp for the timeline
and -2.40 pp for beats. Exact baseline repeats differ by 0.89 pp on average over
all tags (mood 0.72 pp; scene 0.99 pp), with a 2 pp overall p95 and 5 pp maximum.
One repeat per song provides a variation reference, not significance estimates.

No addition establishes an overall quality gain or justifies production adoption.
There are useful individual changes, but rhythm candidates can push plausible
dance suggestions down, and adding all measurements does not preserve the effect
of individual families. Definitions alone change mood probabilities by 1.66 pp
mean absolute difference relative to identical numeric inputs. Unavailable-value
cards also change answers despite retaining the original evidence, so response
changes cannot all be credited to additional measurements.

Withholding only the mood/theme head changes mood values by 5.52 pp mean absolute
difference against all-numeric evidence, with up to 37 pp change. Its remaining
physical/style evidence is not demonstrated to replace musical semantics. The
empty-evidence control returns one mood at 0.54 and no tags at or above 0.70; a
0.50 cutoff alone would therefore admit an unsupported suggestion in this test.

The next useful comparisons are ambiguous rhythm candidates instead of one BPM,
and time-local mood/theme evidence instead of only instrument/style profiles.
Both need independent listening ratings. Keep the compact baseline as the
reference, preserve the mood/theme evidence, and avoid adopting a larger feature
stack from input sensitivity alone. Local `experiment-results.md`, `analysis.json`
and thirteen `song-*.md` reports retain every arm and all 138 values per song.
