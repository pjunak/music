# Audio evidence reassessment after the empty Jev pilot

Research date: 28 September 2026. Source baseline: `fcc32a0`.
This changes the recommended next experiment, not the production contract.
The investigation inspected the completed private pilot, existing audio-model
reference results, current code and upstream documentation, and ran five new
offline synthetic controls. No new paid inference, audio upload or model download
was performed. Private titles, paths and predictions remain in ignored artifacts.

## Conclusion

Prioritize meaningful audio evidence and repair misleading physical summaries.
Further prompt compaction cannot recover instruments, style or emotional character
that the input never represented. The proposed pipeline is:

`audio -> measured facts + learned musical observations -> compact evidence -> Jev -> review`

Jev should interpret musical observations into the owner's vocabulary. It should
not have to reconstruct music perception from brightness and onset statistics.
Its current input is text-only; the vendor also documents difficulties with
numeric reasoning and indirect questions. Keep aggregation and comparisons in
code. [Model specification](https://docs.typesafe.ai/models),
[documented limitations](https://docs.typesafe.ai/model-jaggedness/jev-1.13).

## Confirmed problems and their limits

1. **The complete tagging pilot lacked learned musical evidence.** All 20 native
   executions returned no tags. The underlying 22-recording corpus has no embedded
   artist/album/genre descriptions, no retrieved catalog evidence, and unconfigured
   optional voice inference. No instrument, style, mood/theme or affect model
   predictions reached Jev. Analyzing every second established coverage, not
   musical understanding. See the [completed pilot record](AI_ACCEPTANCE.md#private-jev-input-minimization-pilot-results-2026-09-28).
2. **The rhythm-development representation has a concrete failure.** All 22
   recordings have `rhythmic_drive.shape = volatile`. Fresh, constant-pulse
   60-second controls at 60/90/120/150/180 BPM produce
   `alternating/volatile/steady/mixed/steady`, respectively. These controls do not
   change tempo, pitch or amplitude envelope over time. The implementation counts
   positive onset activity in half-second bins, then measures differences between
   those bins as trajectory variability. Pulse activity and longer-term development
   need different timescales. This is not a reliable descriptor of unstable music.
   [Extraction and trajectory owner](../crates/music-analysis/src/context.rs).
3. **A causal link to Jev is still a hypothesis.** `chaotic` leads the rejected
   scores for every selected real song, while all inputs contain `volatile`.
   That is consistent with a misleading cue, but no paid omission comparison was
   run in this investigation. Also, `major_change_count` is the number of detected
   acoustic section boundaries; it is not an independently validated count of
   major musical transitions. Preserve that distinction in the input wording.
4. **The acceptance suite and real inputs test different capabilities.** Rich
   synthetic descriptions can explicitly state a target mood. Metadata-free
   recordings require perception upstream of Jev. Maintain semantic/safety tests,
   but do not use them as evidence of waveform-to-tag accuracy.
5. **Review suggestions and factual identification have different standards.**
   A scene/setting means useful background for a session, not proof that an actual
   place was recorded. Period means an evoked musical era, not release year. The
   existing harness already says this, but requires specific semantic support;
   supplying only generic measurements leaves little it can use.

## Additional data worth obtaining

### First: instrumentation, musical style and broad mood/theme

Use the existing Discogs-EffNet prototype with its matching MTG-Jamendo instrument
and mood/theme heads. They provide 40 and 56 labels. The encoder also has a
400-style prediction output; our current reference exporter retains embeddings
and the two heads, so style output/label pairing needs qualification before use.
Reuse one encoder pass rather than introducing another model for each task.
[Instrument metadata](https://essentia.upf.edu/models/classification-heads/mtg_jamendo_instrument/mtg_jamendo_instrument-discogs-effnet-1.json),
[mood/theme metadata](https://essentia.upf.edu/models/classification-heads/mtg_jamendo_moodtheme/mtg_jamendo_moodtheme-discogs-effnet-1.json),
[style metadata](https://essentia.upf.edu/models/feature-extractors/discogs-effnet/discogs-effnet-bsdynamic-1.json).

This can distinguish, for example, piano/string/woodwind-led arrangements from
synthesizer-led ones, and provide candidate relaxing, meditative, dramatic or
epic character. These are useful musical observations for both mood and session
suitability. They do not establish historical authenticity or specific instrument
construction; synthesized strings can resemble acoustic strings.

The existing whole-track reference already covers all 22 recordings and 5,503
patches. I recovered the six pilot tracks' time-weighted rankings, verified their
source/PCM hashes and exact head label order, and found differentiated instrument
and theme predictions. These outputs were absent from the Jev pilot. This is a
concrete available input, not evidence that it will improve Jev or match listening.
The [numerical reference ledger](../crates/music-analysis/tests/fixtures/README.md#whole-track-stream-reference-25-september-2026)
and [export/label checks](../crates/music-analysis/tests/fixtures/README.md#original-export-pairing-and-label-identity-25-september-2026)
describe what has already been qualified.

Raw scores need careful handling: top mean mood scores on the six tracks are only
about 0.137-0.287. Applying a universal 0.70 cutoff to these outputs would discard
them again. Retain ranking, raw score type and temporal support; determine any
label-specific thresholds with listening judgments. Ranking alone must not force
a label on unsuitable audio. The mood model's published PR-AUC is 0.14, not a
guarantee of accurate tags here. MTG weights have noncommercial/share-alike terms
and a separate commercial licensing route; a local prototype does not settle
distribution suitability. [Model licenses](https://essentia.upf.edu/models.html).

### Second: audio-to-description matching for the custom vocabulary

Benchmark music-trained CLAP against neutral audible descriptions such as gentle
acoustic folk, sustained dark drones, or forceful orchestral percussion. Use a
fixed, balanced prompt bank with paraphrases and contrasting alternatives.
Calculate similarities locally, retain competing matches and segment agreement,
and give Jev the resulting descriptors with their model attribution. Do not feed
raw embedding vectors or treat a similarity/softmax score as calibrated truth.
[LAION music CLAP](https://huggingface.co/laion/larger_clap_music).

This addresses vocabulary gaps left by fixed classifier labels. It should not
turn an arbitrary fantasy location into an allegedly audible fact. Test direct
session-use prompts separately from musical descriptions; comparisons must not
quietly change both the audio model and the decision question.

For a cheaper deployment challenger, AudioMuse-AI-DCLAP publishes a roughly
7-million-parameter ONNX audio tower using a paired CLAP text tower. Fixed prompt
embeddings could be precomputed when the prompt bank changes. Its teacher is a
specific music/AudioSet CLAP checkpoint, not interchangeable with every CLAP
model. Its speed claims are upstream measurements, and its AGPL terms, native
compatibility and loss of relevant distinctions need evaluation. Reuse a model
only if justified; do not import the whole AudioMuse application.
[DCLAP implementation](https://github.com/NeptuneHub/AudioMuse-AI-DCLAP).

MuQ-MuLan is a further music/text challenger, approximately 700M parameters with
CC-BY-NC weights. Upstream warns that the released training differs from the paper.
It is an experiment if the smaller option misses important distinctions, not an
extra mandatory service. [Official MuQ repository](https://github.com/tencent-ailab/MuQ).

### Targeted additions after those comparisons

- **Perceived valence and arousal:** distinguish subdued-positive from
  subdued-negative character, or energetic-positive from tense-negative character.
  A DEAM-trained model supplies learned affect estimates instead of assigning
  emotional meaning to recording level. The published `deam-msd-musicnn-2` head
  requires its matching 200-dimensional MSD-MusiCNN encoder and outputs valence,
  arousal on a 1-9 scale. It is not compatible with the 1,280-dimensional EffNet
  embedding or the current voice score. Evaluate its extra extraction cost only
  if the first stack leaves a broad affect gap.
  [DEAM model](https://essentia.upf.edu/models/classification-heads/deam/deam-msd-musicnn-2.json),
  [model catalog](https://essentia.upf.edu/models.html#arousal-valence-deam).
- **Real pulse stability and development:** fix the current timescale problem,
  then obtain pulse regularity, beat strength, credible tempo alternatives and
  longer-window changes. Beat This! supplies beats/downbeats, including a small
  checkpoint, if simpler repair is inadequate. Summarize in code; precise BPM alone
  does not decide calmness, urgency or emotion. [Beat This!](https://github.com/CPJKU/beat_this).
- **Harmony and tonal tension:** tonal clarity, major/minor/modal ambiguity,
  sustained drones, sensory roughness and harmonic-change rate may help explain
  tension or stability. Keep these supporting signals; minor is not a synonym for
  sad, and sensory roughness is not the same as harmonic tension. Add these only
  against a named failure, not as a dump of coefficients.
  [Key extraction](https://essentia.upf.edu/reference/std_KeyExtractor.html),
  [dissonance descriptor](https://essentia.upf.edu/reference/std_Dissonance.html).
- **Voice, lyrics and environmental events:** voice coverage is directly useful
  for unobtrusive session music. Transcribe only when intelligible words exist,
  keeping lyrical meaning separate from musical feeling. For actual soundscapes,
  an event classifier such as YAMNet can supply rain, animals or other audible
  event candidates; that is a separate need from classifying instrumental music.
  Avoid routine source separation or transcription for every song.
  [YAMNet sound classification](https://www.tensorflow.org/hub/tutorials/yamnet).
- **A short audio-grounded description:** Music Flamingo is a concrete research
  candidate for instrumentation, arrangement and development descriptions when
  fixed labels and retrieval are insufficient. Its current checkpoint has about
  8B parameters, processes 30-second windows up to 20 minutes, and is licensed for
  noncommercial research. It is a heavier optional benchmark, not the cheap default.
  Ask about audible attributes before showing the desired tags; preserve uncertainty
  and check disputed claims rather than presenting generated prose as verified data.
  [Music Flamingo model card](https://huggingface.co/nvidia/music-flamingo-2601-hf).

Cyanite provides a managed alternative with track/segment mood, instrument and
other outputs. It is useful as an external benchmark if local options fail, with
separate upload consent and provider cost. It is not necessary to implement the
first experiment. [Output contract](https://docs.cyanite.ai/docs/guides/model-outputs/).

## The smallest useful Jev input is a hypothesis to test

Start with a compact summary of leading instrument/style/theme candidates,
trusted relative dynamics, voice status, and only meaningful changes near the
opening, strongest contrasting section and ending. Keep full window results
locally. Each prediction needs its source/model/taxonomy identity, score meaning,
time scope and whether it is calibrated. Correlated heads from one encoder are
not independent confirmations.

Test a roughly 300-800-token musical summary as an initial budget, not a proven
optimum or total request estimate. Tag definitions/questions also consume tokens;
shrinking the song description alone does not remove that cost. Cache extraction
against audio/model identity so vocabulary experiments do not repeatedly analyze
the same waveform. No vector database is required for this experiment.

Implement dedicated learned-prediction observations consumed by both Jev matching
and support checks. Adding arbitrary fields to existing numeric context may simply
be filtered out. Let support consider complementary observations together while
retaining their sources and disagreements. A second Jev judgment over the same
model prediction is not an independent listening verification.

Keep Nouls for individual tag applicability and Choice for an appropriate
single-choice group, including unsupported/ambiguous outcomes. A Noul measures
the probability of answering yes to its exact question, not emotional intensity.
Calibrate review thresholds by task and listening feedback; neither lowering every
threshold nor forcing one tag repairs absent evidence.
[Noul semantics](https://docs.typesafe.ai/primitives/noul).

## Proposed next work, in order

1. Repair and independently test development summaries using fixed-tempo,
   shifted-phase, gradual-tempo and genuine-transition controls. Keep factual
   activity measurements separate from musical-development descriptions.
2. Expose the existing instrument/mood prototype through the offline pilot;
   qualify style output separately. Produce compact, attributed prediction cards
   before adding a production dependency or paid call.
3. Compare corrected physical evidence, learned evidence alone, and their
   combination while holding Jev questions/model constant. Then compare one
   audio/text challenger alone and combined with the best first-stage input.
   Separately test any necessary harness wording change.
4. Include a simple direct classifier-to-vocabulary baseline without Jev. Keep
   Jev only where custom vocabulary, source combination or session-use judgment
   improves results enough to justify it.
5. Obtain owner judgments of proposed and missing tags. Freeze those development
   judgments before selecting thresholds. The remaining 16 recordings are useful
   within-collection confirmation, but these two related albums cannot establish
   broad generalization. Later confirm on other artists/albums and relevant styles;
   unreviewed tags remain unknown, not automatic negatives.
6. Measure useful tag coverage, false suggestions, missing tags, per-group results,
   abstention, processing time, memory and actual provider tokens. Use repeats and
   the same tracks in each paired comparison. Only add affect, harmony, captions
   or further providers for a measured unresolved failure.

The existing safety/conformance suite remains necessary. Model quality requires
listening outcomes as well. All paid comparisons need their own concrete reviewed
plan; this research did not execute or prepare a new paid request batch.

## Local diagnostic artifacts

Ignored directory `target/jev-evidence-research-20260928/` contains
`rhythm_controls.py`, five generated WAVs, `paths.json`, `corpus.json`,
`summarize.py`, `findings.json` and `findings.md`. The latter two combine saved
prediction rankings with new control results; they are not new Jev outputs.
The current `jev-compare pilot-analyze` command performed the offline control
analysis. No runtime source or authored song tags changed.
