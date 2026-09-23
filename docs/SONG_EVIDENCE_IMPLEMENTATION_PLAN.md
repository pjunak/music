# Song evidence and mood tagging implementation plan

Prepared 23 September 2026; scope reviewed against `music` commit `588c9d3`.
Status: proposed implementation; no runtime changes or model certification.
This turns the [research](SONG_EVIDENCE_RESEARCH.md) into dependency-ordered work.
Public model specifications and current source were inspected; native compatibility,
listening accuracy, licensing suitability, and production cost still need the gates below.
All new module names, schemas, commands, and limits below are proposals.

## Product purpose and admission rule

Music is a single-operator, self-hosted music player and tabletop-session orchestrator.
Song evidence should reduce the work of finding suitable music and preparing normal
playlists/sessions. Everyday library use and dependable playback remain primary.
Analysis is optional preparation; playback must never depend on a model or provider.
Follow the existing [Assistant and Authoring workflow](assistant-ux-philosophy.md).

Every addition must name an observed problem, its consumer, a measurable benefit,
and the simplest viable solution. Include setup, compute/storage, provider cost,
maintenance, failure recovery and removal/fallback. Complexity is justified when a
simpler choice demonstrably fails an important use case; an unused field is not progress.

Example outcomes to validate: a quiet exploration bed without disruptive vocals or
climaxes, sustained tension, energetic combat, or a desired listening mood. Evaluate
perceived sound and session suitability separately within existing product workflows.

## Recommended first delivery

Reuse the existing catalog, analysis jobs, structured-output tagger, Mood Library
review, and playlist workflow. Trial one additional audio model family and retain
only the evidence that improves decisions. Jev is an optional comparison on the same
evidence, reflecting the owner's ongoing investigation; it is not a release dependency.

| Component | First delivery | Expand only when |
|---|---|---|
| Evaluation | Small owner-judged, grouped pilot and untouched confirmation cohort | Results are inconclusive, more tags matter, or training needs more examples |
| Audio | Probe Discogs-EffNet with matching mood/theme and instrument heads; adopt only useful heads | A measured failure warrants another model |
| Native inference | Probe `tract-onnx` matching existing Tract; `ort` only if necessary | Compatibility and quality justify native packaging cost |
| Storage | Existing SQLite records plus one bounded learned-feature summary per track/analyzer | An actual consumer needs retained tensors, history, or cross-file reuse |
| Interpretation | Existing tagger with improved evidence; optional Jev comparison | Local training offers a measured quality, offline-use, or cost advantage |
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
[pilot tooling](../tools/mood-pilot.mjs), existing Mood Library review.

- Record a small fixed set of real listening/session requests and current failures.
  Measure useful candidates found, auditioning time, rejected suggestions and review
  effort using the existing planner/review flow. Keep track suitability separate from
  tag accuracy; more tags need not make selection easier.
- Preserve tag IDs, the four groups, and the eight-suggestion limit. Define a small
  core of common tags with positive and confusable examples. Judge every core tag
  per selected recording; do not attempt to certify all 138 default labels at once.
- Keep the existing 30-track pilot as a smoke test. Extend its tooling with a versioned
  JSONL judgment/manifest format and a separate grouped mode; do not silently reinterpret
  its old fixtures. Start with roughly 60-100 representative recordings if available,
  including actual failure cases and a random library sample. This is a pilot size,
  not a statistical claim or a prerequisite for inspecting obvious defects.
- Store stable recording/file references, vocabulary revision, grouping, split seed,
  annotator, listened intervals and blind/assisted status. Keep related versions,
  duplicates and excerpts in one partition. Separate composers/albums where feasible;
  report residual overlap. Use grouped development and untouched confirmation cohorts
  initially; a training/calibration split becomes necessary only for fitted models.
- Label `positive | negative | uncertain | unjudged`; omission is unjudged. Score only
  judged positives/negatives and report judgment coverage. Do not turn all unselected
  tags into negatives. Record whole-track versus excerpt scope explicitly.
- Collect perceived-mood judgments without showing predictions where practicable;
  collect session-use judgments separately. The owner's preferences are the primary
  product target. A second listener can investigate ambiguity; broad multi-listener
  annotation is needed only for claims beyond that owner. Preserve disagreement and
  known blinding limitations. Keep private audio and judgments outside Git.

**Gate:** freeze current configuration and outputs as baseline A before changes;
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
before production adoption.
A failed probe records rejection or justifies one fallback; it does not block useful
catalog/DSP corrections. Benchmark both heads, but ship only heads with a useful consumer.

## 3. Add the smallest evidence contract and storage extension

**Owners:** application `assistant` evidence types,
[storage](../crates/music-storage/src/analysis.rs), and one additive SQLx migration
if the existing storage cannot express the separate learned-feature record.
Keep the eight-crate structure and current dependency direction.

| Record | First-delivery responsibility |
|---|---|
| Existing `track_contexts` | Factual DSP and current source/implementation identity |
| Proposed `track_audio_features` | One bounded record per `(track_id, analyzer_id)`: source/model/preprocessing signatures, coverage/status, aggregate scores, selected intervals, and completion/error facts |
| Existing catalog observations/result JSON | Original claim, source/reference, entity scope, match status and policy revision; extend fields only where the evidence consumer needs them |
| Existing `track_analyses` | Reviewable interpretation with versioned per-tag support/abstention and evidence references |
| Existing `track_user_tags` and review records | Operator-owned accepted state and current review lifecycle |
| Private pilot files | Revisioned manifests, independent judgments and comparison exports |

Keep a typed distinction between measured, catalog-observed and model-predicted inputs.
Represent missing, unavailable, failed, partial and complete states explicitly; missing
is not a zero score. A per-tag decision records support or abstention, bounded evidence
references, contradictions and temporal scope. Raw model scores and optional calibrated
probabilities are different fields; omit calibrated probability until demonstrated.
Retain track-level confidence only as a legacy summary, not a per-tag probability.
Validate finite values, label dimensions, references and payload limits.

Use a proposed 64 KiB maximum learned-feature payload per track as an initial budget,
checked against the actual projection. Store summaries in SQLite; keep full tensors
only for selected research fixtures outside the normal application lifecycle. Introduce
a separate ledger or artifact store only when a concrete query/retention requirement
outgrows existing ownership.

**Gate:** additive migration/doctor compatibility, bounded serialization, missing-data
handling and stale-write rejection. Regenerable evidence can be cleared without changing
accepted tags. Existing database backup/restore remains sufficient for this stage.

## 4. Connect existing sources and preserve correct invalidation

**Owners:** evidence projection;
[catalog workflow](../crates/music-application/src/cleanup_enrichment/workflow.rs),
[typed catalog port](../crates/music-application/src/cleanup_enrichment/catalog.rs),
[catalog invalidation](../crates/music-storage/src/catalog_evidence.rs).

- Begin with the current source signature and exact model/preprocessing/runtime
  revisions for feature reuse. Bind decisions additionally to allowed metadata,
  observation revision, source policy, vocabulary, engine and question/prompt revision.
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

**Gate:** changed-source/model invalidation, ambiguous-match exclusion, source-disable
races, bounded raw-tag handling and duplicate-family grouping. A tag-only file edit
may trigger fresh analysis initially; measure that cost before adding another cache.

## 5. Correct misleading factual inputs and share required preprocessing

**Owners:** [context DSP](../crates/music-analysis/src/context.rs),
[voice frontend](../crates/music-analysis/src/voice.rs), a shared mel module if adopted.

- Extract the existing MusiCNN frontend only as needed by the successful EffNet probe,
  behind parity tests: 16 kHz, 512-sample frames, 256-sample hop, 96 Slaney mel bands
  and log compression. EffNet uses this feature family but **128-frame patches**,
  unlike the voice model's 187. Check centering, downmixing, resampling, silence and
  tails against the pinned reference. [EffNet preprocessing](https://essentia.upf.edu/reference/std_TensorflowPredictEffnetDiscogs.html).
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

- Register a restartable feature-analysis pass in the existing durable job/executor
  infrastructure, defaulting to one worker and a bounded queue. Checkpoint completed
  tracks against their source/model signatures. Cancellation never marks partial work
  complete. Model installation and failure remain independent of server boot, factual
  DSP and playback. Release model memory after the analysis pass.
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
backfilling; reduce batch scheduling or reject the stack if the product budget is exceeded.

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
new evidence must not become hidden local preselection of allowed tags. Version input,
output, analyzer, disclosure, role fingerprint and quality fixtures together. Do not
send additional catalog/learned evidence under the previous consent contract.

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

## 8. Optionally compare Jev through a small typed-decision adapter

**Owners:** [provider inventory](../crates/music-application/src/assistant/providers.rs),
[transport port](../crates/music-application/src/assistant/model_transport.rs),
[HTTP transport](../crates/music-server/src/provider_transport.rs), new `typesafe` handler.
Depends on the shared decision contract. Run this comparison if the owner continues
the Jev investigation; it does not block the existing tagger or stages 9-10.

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
  optional, budgeted follow-up may judge support/contradiction against bounded observation
  IDs. Render application-authored explanations from validated references;
  distinguish considered evidence from evidence the engine actually selected.
- Reuse pre-call checkpoints and usage accounting. Disable automatic retries initially,
  including SDK defaults; uncertain attempts remain interrupted. Test 401/422/429/529,
  timeouts after submission, malformed answers and cancellation. Noul's number is a
  raw vendor probability, not demonstrated calibration on this library. Sufficiency
  gates a decision; do not multiply correlated answers as independent probabilities.

**Gate:** fixture HTTP tests, dedicated conformance, injection/missing-evidence
checks and the same listening/workflow comparison as the current engine. Adopt Jev
only for demonstrated quality, cost or operational benefit.

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

**Gate:** stale-review races, bulk acceptance atomicity, partial-source disclosure,
judgment isolation and focused frontend tests. Generate/validate changed HTTP contracts
and browser guards. No playback wire change is planned; inspect/update Baton only if
an actually consumed shared schema changes. No additional frontend state store.

## 10. Confirm practical benefit, release gradually, and stop when sufficient

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
workflow benefit; inconclusive results do not justify broad rollout or claims about all
138 labels. Drop models that add no value and retain independently useful fixes.

Release through explicit selection: pilot -> selected folders -> resumable backfill.
No automatic paid tagging or silent retagging. Preserve manual tags and the previous
engine; make clearing regenerable evidence independent of authored state. Verify source
withdrawal, database restore and model rollback. Measure concurrent playback under the
current resource budget before broad backfill. A cache or provider outage must leave
ordinary library browsing, playback and authored playlists usable.

Each stage lands as a logical local commit after focused checks. Runtime changes use
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
