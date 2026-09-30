# Jev algorithmic feature investigation, 30 September 2026

Status: both batches complete; owner listening pending. Developer-only experiment; interpretation and
owner listening remain separate from measured signal properties.

## Purpose and sequence

The [previous evidence experiment](JEV_EVIDENCE_RESEARCH.md) established an efficient
138-question batch but found mixed effects from additional classifiers. This
batch asks whether additional physical measurements and a separate affect model
improve decisions, which families matter, and whether compact interval descriptions
work better than precise values.

1. Bind the same 13 exploratory recordings and 138 vocabulary definitions to the
   frozen earlier plan. Keep the six partial owner descriptions out of inference.
2. Extract new whole-recording rhythm, texture and development measurements.
   Check controlled signals, gain behavior, missingness, coverage and the ending.
3. Verify a small independent affect model's frontend, output order and scale;
   preserve source values and parity limits before constructing normalized input.
4. Freeze individual-family, combined-family, interval and label-removal arms.
   Keep the baseline questions, tag definitions and pinned Jev version fixed.
5. Review exact payloads and budget, then run under the owner's explicit test
   authorization. Checkpoint each request before cost; never retry uncertain calls.
6. Export only a complete validated journal. Report costs, repeat variation,
   interactions, negative findings, and changes to known owner-positive tags.
   Commit reproducible tooling; retain private evidence and reports locally.

## Data candidates

| Family | Measurements | Possible use | Interpretation limit |
|---|---|---|---|
| Rhythm | Detected onsets per second, onset-envelope periodicity, inter-onset interval variation | Distinguish steady movement from diffuse texture; support dancing/march/chase comparisons | Note attacks are not beats or verified tempo. Soft onsets, vibrato and half/double-time structure can confuse estimates. |
| Texture | HPSS harmonic/percussive power assignment, pitch-class concentration and changes, spectral flatness, spectral centroid | Describe sustained versus transient sound, pitched development, noise-like texture and brightness | These are neither verified instruments nor mood labels. Pitch-class changes do not establish dissonance, and HPSS is not instrument separation. |
| Development | RMS interquartile range, peak-to-median frame level, ending-versus-opening change, ten time-bin level/onset/centroid summaries | Retain builds, fades and sections that a whole-track average hides | Equal-time bins are not detected musical sections. Relative level is not emotional energy or playback loudness normalization. |
| Affect | Independent learned arousal and valence estimates with verified output scale | Challenge a misleading mood head using a different representation | Learned pleasantness/activation estimates are fallible and cannot determine a setting, era, or personal tabletop use. |

The physical extractor uses 16 kHz mono PCM, a 2048-sample STFT with 256-sample
hops, and a 31-bin median-filter HPSS mask. Frame-center time cells cover every
decoded sample, including the ending. Harmonic/percussive shares assign spectrogram
power to masks; they do not claim source-separated instrument energy. Chroma uses
the harmonic component and reports unavailable values when harmonic evidence is
insufficient. Onset-interval variation is unavailable with fewer than two intervals.
Silence is rejected rather than rendered as a perfectly regular musical signal.

Controlled checks distinguish pulses from sustained tones, regular from irregular
pulses, rising from falling level, and verify gain invariance of reported relative
measurements. They establish measurement behavior, not musical or mood accuracy.

[HPSS documentation and primary references](https://librosa.org/doc/0.11.0/generated/librosa.decompose.hpss.html),
[chroma](https://librosa.org/doc/0.11.0/generated/librosa.feature.chroma_stft.html),
[spectral flatness](https://librosa.org/doc/0.11.0/generated/librosa.feature.spectral_flatness.html),
[onset detection](https://librosa.org/doc/0.11.0/generated/librosa.onset.onset_detect.html).

The separate affect candidate uses the publisher's `msd-musicnn-1` ONNX encoder
(200-value embedding) and `deam-msd-musicnn-2` ONNX head. Its source output order is
valence, arousal; the source range is 1–9, normalized by `(value - 1) / 8` without
clipping. The pinned Essentia.js 0.1.3 frontend matched all 1,152 mel values in the
12-case local reference fixture. Publisher ONNX graphs run directly; parity with
their TensorFlow graphs is not established. Means are unweighted across overlapping
187-frame windows with 93-frame hops and a final ending anchor when needed.
Window variation is descriptive, not a confidence interval. This study does not
use the encoder's direct genre/mood tags as another corroborating source.

[Publisher model contract](https://essentia.upf.edu/models/classification-heads/deam/deam-msd-musicnn-2.json),
[MusicNN encoder](https://essentia.upf.edu/models/feature-extractors/musicnn/msd-musicnn-1.json),
[DEAM dataset](https://cvml.unige.ch/databases/DEAM/).

## Frozen comparison design

Planned primary arms: baseline; rhythm; texture; development; affect; combined
acoustics; combined acoustics in intervals; affect in intervals; all four families;
all four in intervals; all four with the original mood/theme observation removed;
all four with all original learned-label observations removed.

At 13 recordings, 12 arms plus one exact baseline repeat per recording and one
empty-evidence baseline control total 170 requests. Every request contains all
138 frozen short Noul questions. Arm order rotates across recordings. These are
exploratory comparisons; the seven newer tracks influenced design and are not
an untouched holdout. Unmentioned owner tags remain unknown, not negatives.

Intervals use fixed widths in the measurement's own unit and contain the original
value. They reduce precision without silently replacing numbers with mood words.
The raw and interval arms have different information precision; any change must
be interpreted against repeats, not attributed solely to wording. Missing values
remain unavailable in both representations.

Original file hashes, extractor/model identities, source scale/order, auxiliary
hashes, exact request bodies and exact reservations stay in the immutable plan.
Only reconstructed anonymous observation cards go to Jev. Audio, names, paths,
artists, web descriptions and owner feedback remain local. No model result writes
library tags or changes production certification.

## Repeating an offline plan

The developer-only `jev-compare` example adds `feature-plan`, `feature-run` and
`feature-report`, with separate `feature-format-plan`, `feature-format-run` and
`feature-format-report` commands for presentation comparisons. Each refuses to
overwrite its output. The feature input uses
`jev-acoustic-features/v1`, the semantic frozen baseline hash, bounded provenance,
and numeric track IDs joined by the exact original SHA-256. Each record declares
full decoded duration and all four families; arrays have ten values. Nullable
onset variation and chroma values remain unavailable. Unknown fields, mismatched
audio/duration, out-of-range values and plan drift fail before credential access.

```powershell
cargo run --locked -p music-server --example jev-compare -- feature-plan target/baseline.json target/features.json target/feature-plan.json --tracks 1,2
# Use the exact printed hash and reservations, after explicit paid-run authorization:
cargo run --locked -p music-server --example jev-compare -- feature-run target/baseline.json target/features.json target/feature-plan.json --key-file <private-key-file> --plan-sha256 <printed-sha256> --max-requests <printed-count> --max-input-units <printed-units> --output target/feature-run.jsonl
cargo run --locked -p music-server --example jev-compare -- feature-report target/baseline.json target/features.json target/feature-plan.json target/feature-run.jsonl target/feature-result.json
```

This is research tooling that consumes separately produced measurements, not a
production extractor or new application dependency. Private physical and affect
probe scripts, datasets, source checksums, model hashes and reports remain under
the ignored dated `target/jev-feature-research-20260930/` directory. The corrected
affect corpus is under `affect/baseline-bound/`; older parent-directory outputs
used another six-song selection and are excluded. Source-file byte hashes and
native semantic JSON hashes are separate identities and must not be interchanged.

## Primary batch results

All 170 requests completed, returning 23,460 typed Noul answers. Actual usage was
1,403,773 input and 447,270 output tokens: approximately $0.05896 at the current
[published input rate](https://docs.typesafe.ai/models), with free output. The
exact reviewed plan reserved 5,329,260 conservative input units and has semantic
SHA-256 `35c7049d94323511c570fe7b748bcadf281772a4f8663054fc929bf4cd5bc5e5`.

Baseline averaged 7,314 input tokens per song, all raw physical families 8,810,
all four raw families 9,026, and all four interval families 9,776. Intervals
increased input size in this encoding; they are not a demonstrated cost saving.
Planned baseline repeats had mean absolute difference 0.89 percentage points,
95th percentile 2 points and maximum 6 points across all 1,794 paired tag answers.

None of these additions showed a clear gain on the partial owner descriptions:

- The Wedding: baseline dancing 45%, festival 48%; all raw features 37%, 29%.
  The rhythm family alone returned 40%, 34%. Measured pulse periodicity did not
  resolve the known celebration/dance omission under this presentation.
- Arthur's Farewell: calm 80% to 70%, ethereal 62% to 52% with all raw families.
- The Witch of Cymraeg: majestic 66% to 53%; removing the old mood head with
  all new measurements reduced it further to 27%.
- Affect alone moved Hell sirens calm 16% to 30% and Revenge calm 24% to 43%.
  These seven-track changes are suggestions to inspect, not labelled errors.
- Removing all original learned labels left only 1.69 of the baseline's top five
  mood tags on average. It made Hell sirens calm 60%, dark 19%, compared with
  baseline 16%, 81%. The numeric set does not reproduce the baseline distinctions.

The empty-evidence control still returned cold 54%, while no tag reached 70%.
This is another reason not to treat Noul as calibrated musical intensity or
choose a display cutoff from this exploratory panel.

The new measurements passed controlled extractor checks, but that does not
make them demonstrated useful Jev inputs. Their explanatory cards also introduce
definitions and explicit interpretation limits, confounding numeric content with
presentation. The bounded follow-up compared unchanged numbers in detailed and
compact cards, a physical-only compact version, and detailed definitions with
numeric values withheld. Keep the same baseline labels and questions in all arms.
This checks presentation effects before attributing all changes to numeric content.

## Presentation follow-up

The five arms are: exact baseline; exact previous all-family detailed request;
the same 45 raw datapoint slots in compact cards; 43 physical slots in compact
cards without affect; and original detailed cards with every metric value
explicitly unavailable. Compact cards retain metric IDs, units, duration, scalar
precision and missingness, while omitting metric definitions and card type,
representation and interpretation fields. Existing learned observations remain
unchanged. This compares a presentation bundle, not one isolated caution sentence.

The definitions-only control preserves definitions and duration, but removes all
scalar values and arrays. Array shape and numerical precision information are
therefore absent too; it is not simply a same-length prose placebo. It must not
be interpreted as evidence that a musical property is absent.

Five primary arms and an exact baseline repeat for each of 13 recordings, plus
one empty-evidence control, total 79 requests. Order rotates across recordings.
The source 170-request plan's semantic hash is retained and remains reconstructible.
The same offline/run/report argument patterns apply with `feature-format-*`.

All 79 follow-up requests completed, with 641,358 input and 207,849 output tokens
(approximately $0.02694). Its reviewed reservation was 2,428,018 conservative
units; semantic plan SHA-256 is
`4189d43e46da0759ac6fca43bbcfc642fa993979e0db32ed7b83596b0d5a794e`.
Exact baseline repeats had mean absolute difference 0.89 points, p95 2 points
and maximum 5 points. The 249 requests across both batches returned 34,362 Noul
answers and used 2,045,131 input / 655,119 output tokens, approximately $0.08590.
These are published-rate estimates from complete reported usage, not invoices.

| Follow-up format | Input tokens/song | Mean change on 22 partial owner-positive mappings | Mood mean absolute change vs baseline |
|---|---:|---:|---:|
| Baseline | 7,314 | 0 points | 0 points |
| Detailed all families | 9,026 | -7.27 points | 4.51 points |
| Compact all families | 8,326 | -5.00 points | 3.90 points |
| Compact physical families | 8,232 | -4.50 points | 2.86 points |
| Detailed definitions with values unavailable | 8,626 | -6.36 points | 3.75 points |

The positive mappings include tentative moods, use-case/imagery terms, and
interpretive synonyms. They do not constitute a complete labelled dataset, an
accuracy metric, a formal significance test, or a calibrated target intensity.

Definitions/unavailable values alone reduced support almost as much as the full
detailed numeric set. This rules out attributing every reduction solely to the
measured values, but does not isolate one sentence or establish a causal share.
Compact-all uses exactly the same numbers and saves about 700 tokens/song (7.8%)
relative to detailed-all. It recovers some support, with mixed tag-specific effects.

In the follow-up, The Wedding dancing was baseline 46%, detailed 37%, compact-all
43%, compact-physical 40%; festival was 46%, 29%, 31%, 31%. Compact physical data
preserved Heaven Unveiled calm at 85% and The Witch of Cymraeg majestic at 62%
(baseline 66%), versus detailed-all 78% and 53%. On Hell sirens, withholding affect
from compact inputs moved calm 32% to 20%; on Revenge, 33% to 19%. These last two
recordings still need owner ratings rather than an assumed mood ground truth.

## Practical conclusions

No new family demonstrated a clear listening-quality gain. Do not add the entire
stack, replace existing semantic cues with numeric measurements, or automatically
adopt the DEAM estimates. The cheaper baseline remains a useful comparison, not
proof that its omissions are acceptable. The Wedding's celebration/dance failure
remains unresolved.

For further research, use compact, explicitly attributed observations and measure
each added field's value against rated recordings. Small candidates are beat/meter
stability and tempo ambiguity (rather than onset rate), independently measured
sensory roughness/harmonic development (rather than calling chroma change
dissonance), and time-localized affect or instrument/style evidence (rather than
another whole-song mean). These are hypotheses, not demonstrated improvements;
do not add production dependencies for them without a measured benefit.

Private reports retain every tag and low value: `experiment-results.md` and
`song-1.md` through `song-13.md`, plus the presentation counterparts under `format/`.
The primary and follow-up native `result.json` files, descriptive `analysis.json`,
plans and durable journals preserve raw answers, point missingness and reported
cost. `findings.md` provides a concise paired overview for owner listening.
