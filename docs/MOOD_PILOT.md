# Offline mood listening pilot

The September 8 rework makes empty results diagnosable and limits wasted requests.
It does not establish musical accuracy. Use one fixed sample before selecting an
additional local music classifier or scaling text-model tagging.

## Prepare once, without provider calls

Choose 30 varied tracks with current local context: several artists/styles, voice
states, quiet/loud mastering, steady/changing sections and sparse/rich metadata.
Do not use an alphabetical slice of one artist. Store private pilot files outside
Git. Put the selected numeric track IDs in a JSON array, then run from the repo:

```powershell
node tools/mood-pilot.mjs init ids.json cohort.json
```

The command creates a new file, refusing to overwrite an existing one. It assigns
20 development and 10 holdout tracks deterministically, independent of input order.
Listen before viewing model output. For each track, fill `expected_tags` with all
canonical tags you would find useful, and set `reviewed` to `true`. An empty array
is a valid reviewed negative example; an unreviewed track cannot be scored. Keep
the holdout judgments and split fixed while tuning on development tracks.

## Compare retained results

Use **Export retained run results** in the tagging result panel or Batch panel.
The export contains returned track IDs, tags, public explanations, source signatures
and recorded usage. It contains neither audio nor credentials. Returned rows can
include tracks whose changed evidence prevented saving; compare the intended
source signatures and preserve the run context. Older runs without retained rows
cannot be reconstructed by this export. It does not make a provider request.

Use the vocabulary document used for that run, exported from Mood vocabulary. For
an unchanged default vocabulary, the checked-in document works:

```powershell
node tools/mood-pilot.mjs score cohort.json run.json crates/music-application/src/default_vocabulary.json > score.json
node --test tools/mood-pilot.test.mjs
```

The scorer rejects missing judgments, duplicate IDs/tags and unknown vocabulary
names. Reports separate development/holdout results and moods, session uses,
periods and custom groups. They include proposed/useful/false-positive tag counts,
eligible/covered tracks, empty results and missing results. Precision is unknown
when no tags were proposed, not 100%. Missing responses are not abstentions.
Usage covers the whole exported run, even if it contains extra tracks.

The proposed pilot targets are 80% useful-tag precision and 60% coverage of tracks
with a supported target tag. `meets_proposed_targets: null` means insufficient
evidence to assess that group, not a pass. Check mood and session-use results
separately, listen to false positives, and inspect holdout results before scaling.
This sample is a practical decision aid, not a general accuracy estimate.

## Controlled comparison and responsibility

Keep vocabulary, tracks, text model and Thinking fixed. Compare retained original
evidence with compact evidence first, then one local musical-evidence candidate
if needed. Preserve contract/model/source identities and record local elapsed
time, peak RSS and provider-reported input/output tokens; missing measurements
remain unknown. Include any recertification/correction requests in the budget.
The scorer performs no inference or installation and changes no library tags.

Engineering owns model compatibility/profiling, integration and cache correctness.
The operator supplies listening judgments and decides whether suggested uses are
helpful. Paid comparisons and larger library runs remain explicit operator actions.
No local analysis rerun or cleanup certification is needed to inspect old results.

## First candidate compatibility review — 2026-09-08

The established candidate is `mtg_jamendo_moodtheme-discogs-effnet-1`. Its published
head consumes 1,280-dimensional Discogs-EffNet embeddings and produces 56 mood/theme
scores. Its published test PR-AUC is 0.14 and ROC-AUC 0.76 on its own dataset; these
are not accuracy figures for this library. [Exact head metadata](https://essentia.upf.edu/models/classification-heads/mtg_jamendo_moodtheme/mtg_jamendo_moodtheme-discogs-effnet-1.json).

It requires the separate Discogs-EffNet encoder and its preprocessing. The current
voice detector is MusiCNN with two selected outputs; its scores cannot feed this
head, and its internal features are not interchangeable with Discogs embeddings.
The encoder catalog also documents a fixed TensorFlow batch of 64 and an ONNX
alternative. A Rust runtime/preprocessing compatibility and CPU/RSS probe is needed
before proposing a production dependency. [Encoder metadata](https://essentia.upf.edu/models/feature-extractors/discogs-effnet/discogs-effnet-bs64-1.json),
[current voice metadata](https://essentia.upf.edu/models/classifiers/voice_instrumental/voice_instrumental-musicnn-msd-2.json),
[encoder catalog](https://essentia.upf.edu/models.html#discogs-effnet).

MTG publishes its models under CC BY-NC-SA 4.0, with proprietary licensing available.
The application must not silently bundle weights under its own code license.
[Model licensing](https://essentia.upf.edu/models.html).

Decision: retain this as the first bounded experiment, not an accepted runtime
dependency. No weights, new runtime service or classifier predictions were added
by this workflow change. Adoption and scaling depend on compatibility/resource
measurements and useful listening results; neither has been demonstrated yet.
