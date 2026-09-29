# Jev evidence investigation, 29 September 2026

Status: controlled experiment completed. All 119 planned requests returned valid
answers. No production migration, tag writes, or quality certification.

## Questions and work sequence

1. Identify the seven supplied recordings using file metadata and primary sources.
   Keep version ambiguity visible. External descriptions can support or contradict
   broad style claims; they cannot replace the owner's listening judgments.
2. Audit the exact frozen Score and concise-Noul inputs and results. Separate
   upstream classifier errors, lossy evidence selection, question semantics, and
   context that is inherently subjective.
3. Review primary research and usable local models. Prefer evidence improvements
   that reuse existing computation; keep experimental dependencies out of the app.
4. Freeze a paired experiment across six previously rated development recordings
   and the seven unrated recordings. Preserve tag definitions, source hashes,
   pinned Jev version, and raw answers. Compare one factor at a time, with a fresh
   baseline, exact repeats, and missing-evidence controls.
5. Run within the concrete request/input budget under the owner's advance approval.
   Checkpoint before each request; stop on an uncertain outcome without replay.
6. Report descriptive results, costs, source-supported concerns, and remaining
   listening questions. Commit reproducible tooling and public research notes;
   keep private song evidence and reports in ignored local artifacts.

## Initial findings

- The mood/theme head mixes emotion labels with soundtrack and usage labels. Its
  model card reports test PR-AUC 0.14 and ROC-AUC 0.76, not per-track accuracy.
  A Jev judgment built from these labels is a second model's interpretation of a
  fallible classifier, not an independent audio assessment.
  [Official model card](https://essentia.upf.edu/models/classification-heads/mtg_jamendo_moodtheme/mtg_jamendo_moodtheme-discogs-effnet-1.json).
- Whole-track mean top-eight selection discards labels that are salient only in
  a section. The previous temporal recipe adds statistics only for that same
  shortlist, so it cannot recover omitted labels. Section-level selection must
  consider all classes before constructing the small provider card; a single
  maximum is insufficient evidence.
- The two earlier approaches change both primitive and question wording. Noul
  returns the probability of a yes answer; normalized Score expresses a position
  on an ordinal rubric. Differences between their percentages are not accuracy
  gains or changes in measured emotional strength.
  [Noul](https://docs.typesafe.ai/primitives/noul),
  [Score](https://docs.typesafe.ai/primitives/score).
- Broad scene compatibility is not selective retrieval. A scene can accommodate
  many moods. Test a characteristic-choice question while holding observations
  fixed, and preserve raw rankings for the owner to judge.

## Experimental scope

The tested arms are a batched concise-Noul baseline, no mood/theme head,
rank-only learned labels, concise three-level Score, more specific scene/setting
questions, added danceability, added independent acoustic contrasts, and added
section-salient styles. Existing tag IDs and definitions remain authoritative.
One song's questions share a state; recordings are never mixed in one state.

Danceability reuses cached Discogs-EffNet embeddings. It is a new supervised head,
not an independent encoder: its training collection has only 306 recordings.
[Model card](https://essentia.upf.edu/models/classification-heads/danceability/danceability-discogs-effnet-1.json).

Independent audio/text contrasts use the existing cached DCLAP research runtime,
with three paraphrases per side. Only unanimous pair preferences enter the card;
cosine margins remain uncalibrated comparisons. Decode original audio directly
at the model's 48 kHz rate, rather than upsampling the existing 16 kHz PCM.
[Project and inference recipe](https://github.com/NeptuneHub/AudioMuse-AI-DCLAP).

No web descriptions, song titles, artists, story knowledge, or owner ratings enter
this experiment. Research-based assessment stays separate to prevent circular
evaluation. Ratings on the development set can diagnose a failure, but cannot
establish held-out generalization.

## Results

The frozen run covered 13 recordings, eight primary arms, 13 exact baseline
repeats and two missing-evidence controls: 119 requests and 16,422 typed answers.
The reviewed bound was 3,809,901 conservative input units; observed usage was
971,394 input and 307,707 output tokens. At the published $0.042 per million input
tokens and free output, the input-rate estimate is $0.040799. This is an API price
calculation, not an invoice. [TypeSafe pricing](https://docs.typesafe.ai/models).

| Change | Measured result | Interpretation |
|---|---|---|
| All 138 questions in one request per song | Seven-song input fell from 97,629 to 51,141 tokens: 47.6% less than seven partitions. Mean answer difference was 0.86 percentage points; within-run exact repeats averaged 0.89 points, maximum 5 points. | Strong efficiency candidate. The earlier comparison ran at a different time, so this does not isolate batching from temporal variation or prove exact equivalence. |
| Remove mood/theme head | Mean absolute mood change 7.07 points. Dancing improved on an owner-rated celebration recording, but majestic fell 65% to 35% on another owner-rated recording. | The head both helps and misleads. Blanket removal is not supported. |
| Keep classifier ranks, omit response magnitudes | Average mood response rose 2.66 points; mood answers at least 70% rose from 25 to 54 across the 13 recordings. | More confident output is not evidence of better quality; weak model labels can become stronger cues. |
| Add independent audio/text contrasts | About 339 extra input tokens/song. One recording the contrast model preferred as forceful/electronic changed aggressive 35% to 60%, combat 26% to 48%, chase 28% to 47%. | Worth owner comparison, but other-model agreement is not listening ground truth. It did not solve the celebration recording. |
| Add danceability | On the owner-rated celebration recording, dancing fell 44% to 30%. The head ranked an owner-described peaceful recording much more danceable than that recording. | Negative result for this head on the current use case; do not add it by default. |
| Add section-salient style labels | Restored omitted labels, but a candidate period suggestion fell 31% to 18% despite its label being added. Three empty additions were exact no-op controls. | More coverage did not establish better inference. Retain the extraction insight; do not automatically enrich every request. |
| Require a strong characteristic scene/setting match | Scene answers at least 50% fell 205 to 43; setting answers fell 125 to 2 across 13 songs. Explicitly useful owner scene tags also fell. | A different, much stricter task. Fewer suggestions alone are not an improvement. |
| Compact three-level Score | 14,133 input tokens/song versus 7,314 for the batched Noul baseline; rankings changed and some useful secondary descriptions remained weak. | About twice the input of Noul; no demonstrated overall quality advantage. Normalized values are a different quantity. |

The missing-evidence baseline still assigned cold 52% and calm 46%. Neither
control produced a value at least 70%, but a global 50% cutoff would retain an
unsupported mood. Handle absent evidence explicitly and calibrate actual cutoffs
on independent listening judgments; do not subtract these two control runs as a
universal numerical correction. Group comparisons and raw answers stay in the
private local reports. The six partial owner descriptions are development data;
the other seven songs also influenced experiment design and are not a holdout.

Validation: 32 native example tests, 591 workspace tests, 28 existing report tests,
formatting, architecture, workspace check/Clippy, doc tests and generated contracts
passed. Independent read-only review accepted the bounded run. Local extraction
parity and subjective tag quality remain separately limited as described below.

## Reproducible experiment boundary

The native developer example owns plan reconstruction, request limits, journal
validation, and complete raw export. It adds no server setting, production model,
dependency, or automatic tag write. Run from the repository root:

```powershell
cargo run --locked -p music-server --example jev-compare -- ablation-plan BASELINE.json AUXILIARY.json PLAN.json --tracks 1,2,3
# After reviewing the frozen plan under explicit paid-run authorization:
cargo run --locked -p music-server --example jev-compare -- ablation-run BASELINE.json AUXILIARY.json PLAN.json --key-file KEY_FILE --plan-sha256 HASH --max-requests COUNT --max-input-units UNITS --output JOURNAL.jsonl
cargo run --locked -p music-server --example jev-compare -- ablation-report BASELINE.json AUXILIARY.json PLAN.json JOURNAL.jsonl REPORT.json
```

The baseline is a completed, frozen graded-pilot plan. Auxiliary input uses
`jev-audio-auxiliary/v1`, with one record for each selected track, the same original
file hash, and three bounded observations: a finite danceability response summary,
whitelisted unanimous audio/text comparisons, and at most four known section-style
labels outside the baseline top eight. The native validator reconstructs provider
cards from those fields; the auxiliary provenance object remains local. Source
audio hashes are checked again before credential access. Plans, outputs, and
journals refuse overwrites; incomplete or uncertain runs cannot be resumed.

For this experiment the auxiliary extractor reuses all cached 1280-dimensional
Discogs-EffNet window embeddings and all 400 style responses. Nearest-center time
weights cover each decoded sample exactly once. Ten equal-time-bin means select
top-three labels per bin; retain at most four additions by bin count, then mean.
Recomputed whole-recording style means must match the frozen baseline within
`1e-5`. DCLAP independently decodes originals to 48 kHz mono, follows the cached
model's quantization recipe, and uses overlapping ten-second windows including
the ending. Three paired text paraphrases are correlated prompt variants of one
model, not three independent models. Raw responses, coverage, prompt banks and
model hashes remain in private ignored artifacts. This is research extraction,
not an asserted validated production frontend for either model.

## Research implications and useful next work

**Selection comes before adding models.** Sparse uploader tags and an uneven
style taxonomy make a fixed top-eight list a poor universal summary. A bounded
union of whole-track and persistent section-local labels preserves more of the
recording. The first pilot uses ten equal-time bins for an auditable comparison,
not claims of verse/chorus boundaries. Learned pooling is a later option requiring
training data: the auto-pool paper concerns sound-event detection and motivates
testing aggregation, but does not prove this heuristic improves musical mood.
[MTG-Jamendo dataset](https://github.com/MTG/mtg-jamendo-dataset),
[adaptive pooling paper](https://arxiv.org/abs/1804.10070).

**Separate perceived character, scene usefulness, and catalog context.** A film's
plot or game's historical setting is not the musical style of every cue. Store
verified creator/catalog facts with provenance and use them only for decisions
they actually support. Do not force those facts into the audio-derived setting
score. The same applies to a generated caption: it remains a model observation.

**Relative listening judgments may be more useful than precise percentages.**
MusAV uses pairwise arousal/valence comparisons and an independent evaluation
corpus; it illustrates a practical way to obtain less burdensome judgments.
Ask which of two tracks better supports a mood/activity, plus whether a proposed
tag is useful or misleading. Preserve mixed/uncertain judgments. Six partial
owner descriptions cannot calibrate 138 tag probabilities, and unmentioned tags
must not be counted as explicit negatives.
[MusAV paper](https://archives.ismir.net/ismir2022/paper/000078.pdf).

**Prioritize one independent emotional measurement over many correlated heads.**
Musicnn with the DEAM arousal/valence head is a small candidate for the next
controlled experiment. It uses a different encoder from Discogs-EffNet, but its
two affect axes still cannot determine an imaginary location or tabletop scene.
Tempo, voice presence and tonal descriptors are useful only when a concrete
decision needs them; neither minor mode nor quietness establishes sadness/calm.
[DEAM model card](https://essentia.upf.edu/models/classification-heads/deam/deam-msd-musicnn-2.json),
[model catalog and licensing](https://essentia.upf.edu/models.html).

**A bounded audio caption is a stronger semantic challenger, with higher local
cost.** Music Flamingo and MOSS-Music can describe instrumentation, musical
development and emotional character from audio. Test a compact observation
schema on uncertain songs before contemplating a service: prominent sources,
pulse/motion, texture, section changes, and uncertainty; avoid plot and tag
recommendations. MOSS's published caption scores use a model judge and are not
proof of owner-level accuracy. Research on audio-language faithfulness also
shows that fluent explanations can fail to track the audio. These models were
researched but were not installed or benchmarked in this experiment.
[Music Flamingo paper](https://arxiv.org/abs/2511.10289),
[MOSS-Music and its data pipeline](https://github.com/OpenMOSS/MOSS-Music),
[audio-language faithfulness study](https://arxiv.org/abs/2509.22363).

**Large embeddings are optional challengers.** LAION's music CLAP can test whether
distillation limits the cached DCLAP. MuQ-MuLan provides direct music/text matching
but brings a much larger model and noncommercial weight terms. MERT provides
acoustic embeddings without a text tower and needs a trained alignment/head;
it is not a drop-in classifier for the vocabulary. None belongs in production
merely because it is newer or larger.
[LAION music CLAP](https://huggingface.co/laion/larger_clap_music),
[MuQ-MuLan](https://huggingface.co/OpenMuQ/MuQ-MuLan-large),
[MERT](https://huggingface.co/m-a-p/MERT-v1-330M).
