# Song evidence and mood tagging implementation plan

Prepared 23 September 2026 against `music` commit `15722c7`.
Status: proposed implementation; no runtime changes or model certification.
This turns the [research](SONG_EVIDENCE_RESEARCH.md) into dependency-ordered work.
Public model specifications and current source were inspected; native compatibility,
listening accuracy, licensing suitability, and production cost still need the gates below.
All new module names, schemas, commands, and limits below are proposals.

## Recommended implementation

Build a local, versioned song evidence pipeline and compare three decision engines
on the same independently judged recordings: a small local classifier, the existing
structured-output tagger, and optional Jev. Select components by measured usefulness.

| Component | Starting choice |
|---|---|
| Persistence | Existing SQLite/SQLx plus immutable local feature files; existing review transactions |
| Audio baseline | Discogs-EffNet ONNX encoder with its matching mood/theme and instrument heads |
| Native inference | First probe `tract-onnx` matching the existing Tract release; `ort` only if that probe fails |
| Musical language challenger | `laion/larger_clap_music`; benchmark before committing to a native integration |
| Local decision baseline | Separate regularized binary logistic models per supported mood; optional learned source combination |
| Jev | Dedicated typed-decision transport, pinned model, independently calibrated results |
| Evaluation | Grouped listening dataset with explicit negative, uncertain, and unjudged labels |

The fundamental correction is to distinguish measured sound, learned musical
predictions, catalog claims, perceived emotion, and editorial session suitability.
A new provider alone cannot repair missing evidence or an invalid evaluation set.

## 1. Define targets and build the evaluation harness

**Owners:** [vocabulary](../crates/music-application/src/assistant/vocabulary.rs),
[pilot tooling](../tools/mood-pilot.mjs), [CLI](../crates/music-server/src/bin/music-cli.rs).

- Preserve current tag IDs and the four groups. Add a revisioned target definition:
  perceived mood; setting/scene suitability; period feel. Provide positive and
  confusable examples. Keep the eight-suggestion limit initially; retain all internal scores.
- Keep the existing 30-track pilot intact. Add `assistant-mood-dataset/v1` and proposed
  `music-cli mood-dataset {init,validate,import,score}` commands. Use JSONL judgments
  and a manifest containing vocabulary, source/audio hashes, grouping, split seed,
  annotator, listened interval, blind/assisted status, and dataset revision.
- Use `positive | negative | uncertain | unjudged` per assessed tag. Omission is
  unjudged. Score only adjudicated positives/negatives and report judgment coverage;
  never count a rejected suggestion as evidence that every other tag is negative.
- Start with the pilot, then target about 400 distinct recordings and 60/20/20
  train/calibration/test partitions. Keep duplicate/version/excerpt families together;
  group by album/composer where feasible and retain an unseen-composer/franchise
  challenge slice. Report impossible grouping constraints instead of breaking them.
  Tune within grouped training folds; lock the test manifest before comparison.
- Collect blind listening judgments separately from session-use judgments. Double-label
  at least 20% plus ambiguous cases, retaining disagreement. A single listener's
  dataset measures that listener's preferences. Begin with a supported subset of
  common moods; 400 tracks cannot certify all 138 default labels. Fix this core label
  set before predictions and assess every core label per track; other tags remain unvalidated.

**Gate:** deterministic partitions, leakage detection, partial-label scoring,
missing-versus-abstained results, per-tag/group metrics, and recording-group bootstrap
intervals pass fixtures. Freeze the current pipeline's configuration and outputs as
baseline A before changing its behavior. Real baseline collection uses existing consent.

## 2. Resolve native model feasibility before building around it

**Owners:** [analysis crate](../crates/music-analysis/src/lib.rs),
[voice implementation](../crates/music-analysis/src/voice.rs), optional probe binary.

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
| Challenger | `laion/larger_clap_music`: 48 kHz, ten-second inputs, 512-dimensional projected embeddings | Pin checkpoint, processor and tokenizer. First compare upstream results; export audio/text branches and prove native parity only if useful. [Model](https://huggingface.co/laion/larger_clap_music), [configuration](https://huggingface.co/laion/larger_clap_music/blob/main/config.json) |

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
before production adoption.
A failed probe selects a fallback or records rejection; it does not block dataset work.

## 3. Add evidence types and additive storage

**Owners:** new domain `song_evidence` types and application `assistant/song_evidence`
module; [storage](../crates/music-storage/src/analysis.rs) and the next SQLx migration.
Keep the current eight-crate structure and dependency direction.

| Proposed record | Contents and authority |
|---|---|
| `track_audio_identities` | Track/file revision, encoded-file hash, decoded-PCM hash, decoding policy, native format and frame count |
| `song_observations` | Subject kind/ID, recording/version association, claim, original and normalized values, source/reference, source family, time scope, match status, retrieval/policy revision and permitted uses |
| `audio_feature_runs` | Feature key, upstream feature keys, exact model/preprocessing/runtime manifest, completion/coverage, artifact references and performance |
| `track_feature_bindings` | Current track/file revision to reusable feature-run association |
| Existing `track_analyses` | Current reviewable interpretation; add a validated per-tag decision document in its versioned payload, not a second manual-tag store |
| Dataset files | Immutable snapshot manifests, independent judgments and prediction exports; private files outside Git |

Use typed enums for measured, catalog-observed, model-predicted and human-judged
origin. Represent missing, not configured, unavailable, failed, partial and complete
explicitly. Per-tag decisions contain raw score, optional calibrated probability,
calibrator ID, supported/unsupported/abstained status, evidence references,
contradictions and temporal scope. Validate finite values, dimensions, references
and size limits before persistence. Retain track-level confidence only as a legacy
summary, never as a fabricated per-tag probability.

Large arrays use little-endian float32 NPY with a JSON manifest, bounded chunks and
checksums; [npyz](https://docs.rs/npyz/latest/npyz/) provides Rust streaming I/O.
Store files below the configured data root, not the media tree. Write temporary
files, flush and atomically publish, then commit database references. Recovery
removes unreferenced temporary/orphan files; referenced corruption marks features
unavailable. Include artifact backup/restore and a quota; pin benchmark artifacts
against cache eviction. No vector database is needed.

**Gate:** additive migration/doctor compatibility, crash-between-file-and-DB tests,
corrupt artifact handling, quota failure and stale-write rejection. Existing accepted
`track_user_tags` and review behavior survive unchanged.

## 4. Implement audio identity, provenance and dataset ingestion

**Owners:** new evidence service; [catalog workflow](../crates/music-application/src/cleanup_enrichment/workflow.rs),
[typed catalog port](../crates/music-application/src/cleanup_enrichment/catalog.rs),
[catalog invalidation](../crates/music-storage/src/catalog_evidence.rs).

- Use file facts for the existing fast staleness check, encoded hashes for exact
  reuse, and a hash of unnormalized decoded PCM plus native format/decoder revision
  to reuse inference after metadata-only rewrites. A rewrite may still require a
  decode to establish identity. Chromaprint/MBIDs group related recordings; they do
  not justify sharing measurements across remasters, edits or lossy encodings.
- Define `encoder_key = H(audio_identity, encoder, preprocessing, runtime)` and
  `head_key = H(encoder_key, head, runtime)`. A head change reuses the embedding.
  `decision_key = H(feature_refs, metadata_projection, allowed_observations,
  source_policy, vocabulary, engine/question_revision, calibration)` identifies
  interpretation. Recheck track and policy identity inside the write transaction.
- Extend existing catalog observations rather than build another matcher. Retain
  original Last.fm tags/counts before exact-alias filtering, including unmapped tags.
  Reuse MusicBrainz recording/release/work evidence, embedded IDs, AcoustID and imported
  observations. Extend the typed recording response to retain referenced work IDs
  with composer claims. Preserve ambiguity, entity scope and source family; never make an
  artist-level genre or recording match a verified mood.
- Read enabled, current observations through one projection. Disabling a source or
  changing its policy expires dependent decisions. Preserve accepted manual tags.
  Exclude generated suggestions and review history from ordinary model evidence.
- Implement bounded import adapters for selected external datasets; retain IDs,
  audio/excerpt scope, label masks, splits, licenses and overlap groups. External
  examples remain reference data unless an exact local recording/version match is
  established. Download manifests/annotations first, then only selected permitted audio.

| First importer | Purpose and rule |
|---|---|
| [MTG-Jamendo](https://github.com/MTG/mtg-jamendo-dataset) TSV/splits/licenses | Reproduce source-domain behavior and retain weak positive tags; use published 56-label mood split. Missing uploader tags are not verified negatives. Its current terms specify noncommercial research/academic use; audio licenses are per file. |
| [DEAM](https://cvml.unige.ch/databases/DEAM/) annotations/audio manifest | Preserve valence/arousal scales, timestamps and whole-track versus excerpt labels for the optional affect experiment. |
| [OpenMIC](https://github.com/cosmir/openmic-2018) annotations/masks | Validate supporting instrument evidence on judged labels; retain unknowns. |

**Gate:** rename/tag-edit reuse, changed-audio invalidation, ambiguous-match exclusion,
source-disable races and duplicate-family leakage tests pass. Build the larger
listening cohort from these identities; do not treat a model's training dataset as
independent proof of generalization.

## 5. Repair factual DSP and extract reusable preprocessing

**Owners:** [context DSP](../crates/music-analysis/src/context.rs),
[voice frontend](../crates/music-analysis/src/voice.rs), new `mel`/`rhythm` modules.

- Extract the existing MusiCNN frontend behind parity tests: 16 kHz, 512-sample
  frames, 256-sample hop, 96 Slaney mel bands and log compression. EffNet uses this
  feature family but **128-frame patches**, unlike the voice model's 187. Preserve
  voice behavior while checking centering, downmixing, resampling, silence and tails
  against the pinned reference. [EffNet preprocessing](https://essentia.upf.edu/reference/std_TensorflowPredictEffnetDiscogs.html).
- Replace the coarse 20 Hz/integer-lag tempo path with a 100 Hz onset envelope,
  autocorrelation peak interpolation and beat-event interval checks. Retain plausible
  half/double-tempo candidates and an unstable/no-pulse state; do not force a BPM.
  Test 113/127 BPM, tempo changes, rubato and no-beat audio against real annotations.
- Keep absolute loudness as technical evidence. Add relative dynamic range and
  gain-resistant rhythmic/spectral descriptors. Do not feed the current loudness-
  dominated intensity proxy into a classifier as independent evidence of arousal.
  Add chroma/tonal-change features only as an ablation; key mode is not a mood label.
- Version changed factual context semantics and implementation identities. Learned
  mood/instrument outputs live in `audio-features/v1`, not factual `local-context`.

**Gate:** gain-change, clipping, silence, short-file and tempo regression fixtures;
no regression in current voice inference. Compare DSP error on labeled audio, not
just synthetic metronomes. Export measured coverage separately from calibrated accuracy.

## 6. Build bounded feature extraction and temporal aggregation

**Owners:** new analysis `features` module, application `audio_features` port/job,
[server composition](../crates/music-server/src/analysis.rs), existing analysis executor.
Depends on stages 2-5.

- Add a restartable `assistant.library-feature-analysis` job, defaulting to one model
  worker and a bounded queue. Stream into small chunks; checkpoint completed tracks
  against feature keys. Cancellation never marks a partial track complete. Keep
  optional model failure independent of factual DSP and playback.
- Run the chosen EffNet encoder once per patch, then its matching heads. Save
  raw embeddings, all raw scores, actual intervals, padding/valid duration and model
  identities. Use the documented 62-frame hop initially. Handle short/tail patches
  explicitly and report padded coverage. Whole-track-trained heads produce window
  estimates; they do not establish precise human-labeled mood boundaries.
- Aggregate overlapping outputs by actual time support into ten-second bins and
  whole-track mean, variation and upper quantiles. Retain transitions, sustained
  support and contradicting passages. A brief climax must not label the entire track.
  Bound provider projections independently of stored detail.
- CLAP comparison uses deterministic ten-second windows with five-second hops,
  including the ending, and cached text embeddings for tag definitions plus confusable
  alternatives. Its published processor defaults to random truncation; replace that
  selection policy with explicit windows, retaining model preprocessing. Similarity
  remains uncalibrated. Decode at 48 kHz from the source, not upsampled 16 kHz context.
  [Processor settings](https://huggingface.co/laion/larger_clap_music/blob/main/preprocessor_config.json).
- Persist full window features for the benchmark first. Estimate library storage and
  runtime from those measurements before choosing retention or pooling. Quantization,
  reduced sampling and shared decoding are later optimizations with parity/quality gates.

**Gate:** deterministic reference export, full/tail coverage, bounded long-file
memory, interruption/resume, stale-file races, cache hits and disk exhaustion pass.
Publish seconds per audio minute, bytes per track and peak RSS for each candidate.

## 7. Build the shared evidence projection and local decision baseline

**Owners:** new application `mood_evidence`/`mood_decisions` modules;
[tagger](../crates/music-application/src/assistant/model_tagger.rs) and dataset tooling.

Create one `song-evidence/v1` projection for all interpreters: allowed metadata,
source-attributed observations, musical predictions, temporal summaries, missingness
and conflicts. Exclude titles, filenames, paths, existing suggestions, manual tags
and listening-test labels. Raw identity records stay local. Feature names, units,
score meaning and source family travel together. Apply source-use permissions before
projection. Never send embeddings as unexplained numbers to a text model.

Implement independent L2-regularized binary logistic heads for supported moods using
`linfa-logistic` in an offline CLI feature. Begin with learned head scores and DSP;
measure the extra value of pooled embeddings. Export coefficient/scaling manifests
and use simple Rust inference in the server. Train only on independently judged
positives/negatives; mask uncertain/unjudged entries. Bind exported models to their
training manifest, vocabulary and permitted-use policy. Do not use multinomial softmax
for coexisting moods. [Rust binary logistic implementation](https://rust-ml.github.io/linfa/rustdocs/linfa_logistic/type.LogisticRegression.html).

Use grouped training folds for feature selection and regularization. If combining
an audio classifier, catalog features and an interpreter helps, fit a small combiner
on out-of-fold predictions, with missing-source indicators. Do not average vendor
confidence values or count correlated heads/copied tags as independent votes.
Calibrate on the separate calibration partition using a sigmoid mapping first;
retain raw scores and per-tag abstention thresholds. Sparse labels stay experimental.
Evaluate reliability diagrams alongside Brier/log loss: Brier alone mixes calibration
and discrimination. [Calibration reference](https://scikit-learn.org/stable/modules/calibration.html).

**Gate:** replayable training/export/inference parity, no test-label access,
missing-source tests and per-tag calibration reports. Compare audio-only,
metadata-only and combined evidence before adopting the combiner.

## 8. Add Jev as an interchangeable decision engine

**Owners:** [provider inventory](../crates/music-application/src/assistant/providers.rs),
[transport port](../crates/music-application/src/assistant/model_transport.rs),
[HTTP transport](../crates/music-server/src/provider_transport.rs), new `typesafe` handler.
Depends on the shared decision contract; it need not wait for local classifier training.

Add `TypedDecisionTransport` and `typed-decisions/v1`. Let the music-tagger role
select a structured-text or typed-decision engine with matching conformance tests;
keep other roles' capability requirements intact. Jev is not an OpenAI-compatible
chat adapter and must not claim arbitrary structured-text or audio support.

Use direct `reqwest` HTTP with the existing credential, URL, byte/time and attempt
boundaries. Implement `POST /v1/systemone` with `{model,state,questions}` and strict
`answers` parsing; validate exact question keys/types, finite scores and model ID.
Discover models from `models[].name`, rather than the existing `data[].id` parser.
Allow an explicit pinned `jev-1.13.0` even when discovery lists only aliases.
[API](https://docs.typesafe.ai/api), [models and discovery](https://docs.typesafe.ai/models).

- One track per shared state initially. Use Noul for each nonexclusive tag and a
  separate evidence-sufficiency question; Choice only for period with unknown/cross-era
  options. Put full definitions in questions, since question IDs carry no semantics.
- Deterministically partition the entire vocabulary into bounded question groups.
  Enforce both documented context limits with conservative reservation units and show
  total calls/cost before running. Never silently omit custom tags or assume one call
  handles a 1,200-tag vocabulary. Cache by exact state/questions/model identity.
- Keep aggregation, thresholds and consistency in Rust. For proposed tags, an
  explicitly budgeted follow-up may judge support/contradiction against bounded supplied
  observation IDs. Render application-authored explanations from validated references;
  distinguish considered evidence from evidence the engine actually selected.
- Reuse pre-call checkpoints and usage accounting. Disable automatic retries initially,
  including SDK defaults; uncertain attempts remain interrupted. Test 401/422/429/529,
  timeouts after submission, malformed answers and cancellation. Noul's number is a
  raw vendor probability, not demonstrated calibration on this library. Sufficiency
  gates a decision; do not multiply correlated answers as independent probabilities.

**Gate:** fixture HTTP tests, dedicated conformance, injection/missing-evidence
checks and the same listening comparison as other engines. Refit any optional
combiner/calibrator on development data after adding Jev, without reopening test tuning.

## 9. Integrate decisions, disclosure and review

**Owners:** [tagging jobs](../crates/music-application/src/assistant/model_jobs/tagging.rs),
[atomic review](../crates/music-storage/src/assistant/review.rs),
[Assistant HTTP DTOs](../crates/music-server/src/assistant/mod.rs),
[review UI](../frontend/src/views/assistant/AnalysisTagReview.tsx).

Version the tagger input/output, analyzer, role fingerprint, disclosure and quality
fixtures together. Adapt the existing structured-output tagger to the shared evidence
and per-tag contract. Do not pass new catalog/learned evidence under old consent.
Keep its current full-vocabulary classification route as a baseline; proposing a
local classifier is an explicit new engine, not hidden tag preselection for that route.

Show measured versus predicted evidence, source, age, model, usable audio coverage,
per-tag support, contradictions and abstention. Add playback links to supporting
intervals. Keep raw scores distinct from calibrated confidence. Recheck the complete
decision identity during acceptance, including source policy, feature runs, vocabulary
and engine/calibrator revisions. Only explicit review writes `track_user_tags`.

Add a separate blind listening view or export mode: anonymized IDs, no metadata or
predictions, interval playback and four-state judgments. Assisted corrections retain
that status and do not automatically become locked benchmark labels.

**Gate:** stale-review races, bulk acceptance atomicity, partial-source disclosure,
blind-label isolation and frontend tests. Generate/validate changed HTTP contracts
and browser guards. Playback wire changes are unnecessary; inspect/update Baton only
if an actually consumed shared schema changes.

## 10. Select, release gradually and measure

Nominate the primary configuration using development results; freeze manifests,
thresholds and its baseline comparison before opening the locked test results.
Compare baseline A, improved evidence with the current tagger, the local classifier,
and Jev; include CLAP only after its feasibility gate. Treat additional test-set
comparisons as exploratory; a test-informed redesign needs a fresh confirmation set.
Use identical tracks and judgments. Report per-tag precision/recall/PR-AUC,
useful-track coverage, abstention,
judgment counts, calibration, latency, storage and provider cost. Separate mood from
session suitability and period. Run no-audio, shuffled-audio and source-removal controls.

**Proposed promotion targets:** retain at least 0.80 precision and 0.60 useful-track
coverage on adequately judged target labels; seek >=5 percentage points more coverage
at matched precision, with recording-group confidence intervals. Insufficient samples
or inconclusive improvement means more evaluation, not a claim of success. Do not
certify rare tags from pooled averages. Preserve the existing semantic/safety gates.

Ship behind explicit feature/engine selection: pilot -> selected folders -> resumable
library backfill. No automatic paid tagging or silent retagging. Keep the previous
engine available, preserve manual tags, and make feature cache deletion independent
of authored state. Verify backup/restore, source withdrawal and model-version rollback.
Backfill must fit the current container budget without material playback regression;
measure that with concurrent playback before enabling it broadly.

Each stage should land as a logical local commit after its focused tests. Runtime
changes then run the applicable [validation matrix](VALIDATION.md): Rust and contract
gates, frontend gates for visible changes, both dependency lockfiles and license checks
for new crates, plus container checks for inference packaging. Mark paid-provider,
licensed-model and listening evidence separately from CI fixture success.

## Extensions only after a measured gap

| Observed gap | Concrete next implementation |
|---|---|
| Affect dimensions remain weak | Benchmark `msd-musicnn-1` plus `deam-msd-musicnn-2` separately; the head expects 200 values and outputs `(valence, arousal)`. It cannot consume EffNet or the voice classifier's two scores. [Exact head](https://essentia.upf.edu/models/classification-heads/deam/deam-msd-musicnn-2.json) |
| Rhythm still unreliable | Export/probe [Beat This!](https://github.com/CPJKU/beat_this), retaining no-beat/rubato evaluation and resource gates. |
| CLAP misses soundtrack semantics | Compare [MuQ-MuLan](https://github.com/tencent-ailab/MuQ) on the same cohort before accepting its larger runtime and weight-license constraints. |
| Catalog coverage is insufficient | Add Discogs edition/credit and referenced Wikidata work relationships through typed observation adapters; prioritize measured missing fields, not unrestricted scraping. Lyrics need a separate permitted, versioned text channel. |
| Too few difficult examples | Add active-learning selection from uncertainty/disagreement plus a random audit fraction; keep recording groups and frozen tests protected. |
| Local quality remains insufficient | Benchmark [Cyanite](https://docs.cyanite.ai/docs/intro/) behind the dedicated audio-upload consent/job contract. It is not part of the initial text-only Jev integration. |

Do not begin with fine-tuning a large encoder, training on generated tags, a vector
database, or several production analysis services. The immediate deliverable is a
reproducible dataset and a demonstrably better, replaceable evidence-to-tag pipeline.
