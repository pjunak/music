# Song evidence and mood tagging implementation plan

Prepared 23 September 2026; clean-cutover scope reviewed against `music` commit `046f67e`.
Status: core implementation complete; independent listening and production acceptance remain open. No model is certified.
This turns the [research](SONG_EVIDENCE_RESEARCH.md) into dependency-ordered work.
Public model specifications and current source were inspected; native compatibility,
listening accuracy, licensing suitability, and production cost still need the gates below.
The status below identifies delivered contracts; conditional stages remain proposals.

## Implementation status — 25 September 2026

- **Implemented:** selected-library inventory and saved-vocabulary exports initialize
  the grouped JSONL pilot before model calls. Frozen recording groups, four-state
  labels, development/confirmation scoring, per-tag counts, known-positive recall
  and paired group-bootstrap comparisons remain. Empty/failed run exports preserve
  missing outcomes; model-success-based initialization is removed. Freezing works
  before listening. The owner has no labeled dataset yet; listening remains open.
- **Implemented:** context v3 with gain-invariant relative dynamics, explicit coverage,
  no whole-track context confidence, `voice_score`, and coarse local tempo withheld from
  the model projection. Existing bounded execution and source-audio decoding are reused.
- **Implemented:** bounded original Last.fm observations, current-policy MusicBrainz/
  Last.fm projection in tagger input v24 (`song-evidence/v1`), source-aware result identity,
  transactional save/review guards, updated disclosure v15 and runtime fingerprints.
- **Implemented:** forward schema-15/16 reset of generated analysis and proposal reviews,
  old-job supersession and audit preservation, current-only context parsing, updated
  existing review/inspector UI. Accepted/manual tags and authored playlists survive.
- **Implemented:** read-only factual and voice acceptance probes using the real extractors,
  with bounded repeats, explicit cancellation outcomes and shared input/memory helpers.
  Voice reports readiness, per-track work and each worker start/join separately; scores
  appear only for completed classification. No model, dependency or storage layer was
  added. Production container totals and concurrent playback need separate measurement.
- **Implemented:** bounded final loudness-report capture survives verbose embedded notes;
  factual decode/loudness codec and filter pools are limited. The acceptance probe exposes
  numeric measurements. The direct ebur128 substitution failed end-of-file/short-signal
  checks and was rejected; no faster loudness algorithm has been adopted.
- **Implemented:** shared MusiCNN frame preprocessing with independently generated,
  checksum-pinned numerical fixtures and reusable FFT scratch. All 1,152 synthetic
  frame features pass the fixed tolerance; the frame transform definition is unchanged.
- **Implemented:** complete ending windows and constant-storage voice summaries, strict
  invalid-value/cancellation handling, normalized stereo input and bounded FFmpeg pools.
  The exact pinned graph and real FFmpeg/worker tests now run; decoder/window identities
  make older generated contexts stale without changing accepted/manual tags.
- **Implemented:** each voice worker verifies the exact owned model bytes it parses,
  with bounded input and replacement/deletion/recovery regressions. The artifact/v2
  identity retires contexts produced under the previous startup-only verification.
- **Native probe:** five synthetic patches and 66 beginning/interior/ending patches
  from 22 owner-approved recordings pass fixed feature/ONNX numerical gates. A
  bounded development exporter preserves silent-frame positions and verifies the
  pinned references/models. Complete streaming/tail aggregation, original TensorFlow
  equivalence, cancellation, production resources and listening usefulness remain
  open; no new application model runtime or weights are bundled.
- **Implemented:** tagger output v5 / analyzer v8 with per-tag support, reasons,
  validated supporting/conflicting observation IDs and explicit abstention. Current-only
  save/review contracts, strict fixtures, disclosure and existing review UI are updated.
- **Implemented:** retired metadata/audio heuristic jobs, routes, schemas and UI; removed
  their saved axes and readers. Automatic playlists use accepted/manual tags. Playlist
  ranking retains metadata search but no longer presents keyword guesses as analysis tags.
- **Still required before production acceptance:** independent owner judgments,
  real-library listening comparison, resource/concurrent-playback checks, and an
  operator-started production rebuild. No paid calls or production changes were made.
- **Conditional:** learned audio integration follows its parity/usefulness gates. Jev,
  training, extra encoders/datasets and a new annotation UI are not release dependencies.

### Local validation and release boundary

Across completed batches, Windows GNU validation passed 519 Rust tests, workspace
and fuzz Clippy, formatting, architecture, generated contracts, doctor/migration
coverage and the headless release build. Earlier batches also passed all 361 frontend
tests, frontend lint/typecheck/production build and 23 grouped mood-pilot tests.
This development-reference batch passed 10 new reference-tool tests and 8 workflow
policy checks; the existing 1,152-value pinned frame fixture remains identical.
The migration test starts with the old schema, verifies the backup/reset, parses a
preserved automatic rule with its new tag source, and confirms fresh results survive
reopening. Browser guards reject the retired result shape.

No Docker host was available for the production-image smoke test. Visual inspection
could not initialize because the Codex browser helper failed with a local ACL error;
component interaction tests passed. Owner listening, production resource/concurrent-
playback checks and the live rebuild remain separate acceptance work. No private
library judgments were invented, and no provider calls, push or deployment occurred.

### Batch progress and tool inventory

**Latest batch:** made the optional EffNet numerical experiment usable on real audio.
The pinned Essentia.js convenience FrameGenerator drops silent frames; it removed
1-59 frames in 20 of the 22 approved recordings. Indexing its output as a complete
timeline would shift later patches. A small development-only reference exporter
now cuts centered frames explicitly, retaining silence, real sample offsets and
boundary padding. It shares the pinned Essentia loader with the existing fixture
generator, verifies the ONNX reference/model artifacts and refuses output overwrite.

The tool selects beginning, interior and ending patches from bounded private 16 kHz
mono PCM. All 66 patches passed the unchanged feature, embedding and head gates
against the shared Rust preprocessing and isolated Tract probe. This establishes
selected-patch parity on common decoded input, not decoder or complete wrapper
equivalence. Original audio hashes, sizes and modification times stayed unchanged.

Validation: 10 reference-tool tests and 8 workflow-policy tests passed; existing
synthetic fixtures reproduced exactly. Real-tool controls rejected overwritten
outputs and modified weights, and a deliberately perturbed reference failed the
numerical gate. CI runs only the dependency-free unit tests. Application runtime,
frontend, Rust dependency graphs, schemas and provider behavior are unchanged;
their earlier gates were not rerun. See the
[reference ledger](../crates/music-analysis/tests/fixtures/README.md#real-audio-patch-reference-25-september-2026)
for commands, observed errors and the framing limitation.

Independent listening and real-library candidate usefulness remain open. EffNet
also needs full streaming/aggregation, original TensorFlow-export equivalence and
resource/cancellation qualification before adoption. Docker and WSL are unavailable
on this host, so production resource/concurrent-playback measurements and the
operator-started rebuild remain separate gates. No application model, audio upload,
paid call, authored-tag change or deployment was added.

For every subsequent batch, update the delivered work, checks, remaining gate and
this inventory. Importance reflects this product's needs, not model popularity.
Conditional items are options requiring an observed failure and a measured benefit;
they are not all scheduled for implementation. The original research is a dated
options survey; this plan determines the narrower implementation scope.

| Tool or approach | Old use | Current use | Planned decision | Importance and value; reason |
|---|---|---|---|---|
| Metadata-keyword mood analyzer | Title/genre/album guesses | Removed | Keep removed | Remove: lexical associations were not independently grounded mood evidence. Ordinary metadata search remains useful. |
| Audio energy/brightness/tension mood rules | Heuristic generated tags and saved axes | Removed | Keep removed | Remove: loudness and spectral measurements do not establish emotional meaning. |
| FFmpeg / ffprobe | Decode and technical inspection | Bounded pools, reliable loudness capture and normalized voice downmix | Keep | Core: correct input levels and complete original-audio evidence; stereo defaults previously boosted voice input by about 3 dB. |
| FFmpeg loudnorm / ebur128 | Loudnorm input measurements | Loudnorm retained; direct scanner comparison failed | Keep loudnorm until independently validated replacement | High correctness priority: a faster scanner missed an ending peak and disagreed on short-signal range. No speedup claim. |
| music-context-probe / music-voice-probe | No factual acceptance CLI; basic single-pass voice probe | Current v2 reports; bounded repeats/cancellation, factual coverage and voice worker lifecycle | Keep for rebuild acceptance | High: measure actual original-audio extraction and cleanup before a large rebuild; process RSS is not whole-container evidence. |
| RustFFT factual context | Older DSP and global confidence | Context v3, relative dynamics and coverage | Keep and measure | Core: local changes, endings and dynamics can help reject unsuitable session music; confidence is not inferred from duration. |
| Coarse tempo estimator | 20 Hz integer-lag estimate | Local inspection only; omitted from tagger evidence | 100 Hz onset/interpolation only if rhythm errors matter | Conditional: improve pulse accuracy when it changes actual selection; no rhythm project by default. |
| MusiCNN voice classifier + tract-tensorflow | Optional local voice estimate | Ending coverage, bounded summaries and exact model snapshot verification per worker | Keep optional | High session value: audible vocals need correct input and dependable model attribution; window scores remain uncalibrated. |
| Essentia.js / ONNX Runtime Web references | Synthetic frame/patch checks | Pinned offline references; explicit framing preserves silence and ending offsets | Keep development-only; reject FrameGenerator as a timeline oracle | High: avoid validating shifted patches; the convenience helper drops silent frames. No production dependency. |
| AcoustID / Chromaprint | Recording identification | Existing conservative identity matching | Keep | Core for source matching: prevents attaching facts to the wrong recording; does not verify mood or equivalent editions. |
| MusicBrainz | Recording/catalog enrichment | Current-policy recording genres, composer/date claims in shared evidence | Keep | High: attributable recording context without pretending catalog genres are listening judgments. |
| Last.fm | Community tags, exact vocabulary mapping | Bounded original tags/counts with weak-source attribution | Keep bounded | Supporting: useful descriptors and vocabulary, but community counts are neither ground truth nor independent votes. |
| Structured text model tagger | Whole-track confidence and tag list | Per-tag support, evidence/conflict references and abstention | Keep with review | Core optional interpretation: combines permitted evidence with the owner's vocabulary; never writes accepted tags itself. |
| SQLite / durable jobs / review guards | Existing persistence and execution | One-way generated-data reset; current evidence identities | Keep | Core safeguards: resumable local work, stale-result rejection and preservation of authored state. |
| JSONL listening pilot + grouped bootstrap | Small pilot, then inventory derived from successful results | Explicit selected inventory, saved vocabulary, frozen groups, paired comparison and empty-run exports | Collect independent judgments; evaluate development, then confirmation | Essential: prevents success-only selection and distinguishes useful tags, abstentions and missing outcomes without a dataset application. |
| Discogs-EffNet + matching MTG-Jamendo mood/theme and instrument heads | Not used | Five synthetic and 66 real-audio patches pass fixed feature/ONNX gates | Qualify full extraction, TensorFlow equivalence, resources and usefulness before adoption | Conditional high value: richer evidence; common-PCM patch parity does not certify complete inference or mood quality. |
| tract-onnx / ort | Neither in production | Tract 0.23.7 passes isolated synthetic and real-patch comparison | Prefer Tract if fully qualified; native ORT only if required | Conditional infrastructure: retain one production runtime; the WASM oracle is development-only. |
| TypeSafe Jev | Not used | Owner exploration; no adapter | Optional typed-decision comparison on the same evidence | Conditional: retain only for measured quality/cost value; not a chat-compatible replacement or release dependency. |
| LAION larger_clap_music | Not used | Research option | Compare only for a remaining semantic/retrieval gap | Deferred: flexible text/audio matching; similarity is not probability and runtime cost must be justified. |
| MSD-MusiCNN + DEAM head | Not used | Research option | Probe only if affect dimensions remain weak | Deferred: valence/arousal evidence; requires its own matching encoder, not the existing voice output. |
| Beat This! | Not used | Research option | After a demonstrated failure of simpler rhythm repair | Deferred: beat/downbeat detail only when useful to selection; adds native integration and resource work. |
| L2 logistic heads (linfa-logistic), source combiner and calibration | Not used | No trained local model | Only with sufficient independent grouped labels and a quality/offline/cost need | Deferred: a small local alternative may help; do not train on generated tags or average unrelated scores. |
| MTG-Jamendo / DEAM / OpenMIC datasets | Not imported | Reference datasets only | Narrow import for a specific label/domain question | Conditional: preserve partial labels, splits, licensing and version scope; not substitutes for owner judgments. |
| MusicBrainz work relations / Discogs / Wikidata | No added analysis adapters | Research options beyond current projection | Add only a missing fact with a useful consumer | Conditional: extend attribution without collecting unused catalog fields. |
| Content hashes / NPY artifacts (npyz) / embedding cache | File facts used for freshness | No new tensor store; pilot accepts private content references | Add only for measured invalidation, reuse or training needs | Deferred: cache maintenance and storage need a demonstrated saving or consumer. |
| MuQ / MuQ-MuLan / Cyanite | Not used | Research challengers | Only if smaller choices fail an important use case | Low current priority: larger local resources or explicit external audio upload and recurring cost. |
| All-In-One structure analysis | Not used | Research reference | Not in delivery scope | Low: pop-section semantics and source-separation cost have no demonstrated tabletop benefit. |
| Annotation UI / active learning / vector database / large-model fine-tuning | Not used for this rework | No new platform | Excluded by default; reconsider only a concrete bottleneck | Avoid bloat: pilot files, existing review and SQLite cover current needs. |

### Native probe evidence

An isolated release-build Rust probe on the Windows GNU host loaded all three artifacts.
The ONNX encoder's real output names are `activations` and `embeddings`, and both heads
use `activations`; the catalog JSON names describe a different exported graph interface.
Use `with_ignore_value_info(true)` to let Tract infer intermediate shapes after binding
batch size one; otherwise symbolic `batch_size` value-info conflicts with the concrete input.
No graph operations or weights were rewritten. Encoder input is `[1,128,96]`, embedding
`[1,1280]`, head inputs `[1,1280]`, outputs `[1,56]` and `[1,40]` respectively.

| Artifact | Verified SHA-256 |
|---|---|
| `discogs-effnet-bsdynamic-1.onnx` | `a280825b334797cf677939db8cd5762c0392aedd0ca6415dbc1cd083f045e43c` |
| `mtg_jamendo_moodtheme-discogs-effnet-1.onnx` | `7d6270acaa5f4bba4b115a0d6849aca05ed6bd153dcb6d9da4f6ab9f99ef10ff` |
| `mtg_jamendo_instrument-discogs-effnet-1.onnx` | `9ae2d9e763d66bd8eed654d1ac3aa171e6539cb8a0e11f3dcd53df1428980802` |

The initial zero-input smoke test was supplemented by five synthetic 128-frame
patches. Essentia.js 0.1.3 supplies reference mel features and ONNX Runtime Web 1.30.0
(single-thread CPU WASM) runs the same published graphs. Both graph-only comparison
and the shared Rust frontend plus Tract meet the fixed feature/embedding/head gates.
All values are finite with the expected dimensions; worst mel error is 0.0000290871
and worst head error is below 0.00000090. See the
[reference ledger](../crates/music-analysis/tests/fixtures/README.md#isolated-effnet-comparison-24-september-2026)
for the cases and limits. This is controlled numerical evidence, not original
TensorFlow-export equivalence, real-music coverage, production resource acceptance
or mood-quality evidence. The artifacts and temporary native probe remain ignored
research output, not application dependencies. Upstream links and licensing remain in stage 2.

The 25 September real-audio extension passed 66 selected patches from the two approved
albums using the same gates. It replaces the silence-dropping convenience frame helper
with explicit positions in the shared decoded PCM. The
[reference ledger](../crates/music-analysis/tests/fixtures/README.md#real-audio-patch-reference-25-september-2026)
records the new exporter, 811,008 feature comparisons and remaining complete-path limits.

## Product purpose and admission rule

Music is a single-operator, self-hosted music player and tabletop-session orchestrator.
Song evidence should reduce the work of finding suitable music and preparing normal
playlists/sessions. Everyday library use and dependable playback remain primary.
Analysis is optional preparation; playback must never depend on a model or provider.
Follow the existing [Assistant and Authoring workflow](assistant-ux-philosophy.md).

Every addition must name an observed problem, its consumer, a measurable benefit,
and the simplest viable solution. Include setup, compute/storage, provider cost,
maintenance, failure recovery and removal. Complexity is justified when a
simpler choice demonstrably fails an important use case; an unused field is not progress.

Example outcomes to validate: a quiet exploration bed without disruptive vocals or
climaxes, sustained tension, energetic combat, or a desired listening mood. Evaluate
perceived sound and session suitability separately within existing product workflows.

## Clean cutover: recompute all generated analysis

Replace the analysis contract and rebuild every indexed recording from its source audio.
Remove superseded analyzers, schemas, parsers, aliases, pilot formats, compatibility
branches and old-engine fallback. Do not translate or reuse old generated results.
Only results produced by the new pipeline can satisfy analysis or review freshness.

Preserve source files, embedded metadata, track identities, attributable catalog facts,
accepted/manual tags, independently collected judgments and authored playlists/campaigns.
Refresh external observations under current source policy. Clear derived contexts,
features, predictions and proposal-bound review state; retain paid-attempt accounting
only as non-executable audit history. A failed or unfinished rebuild remains visibly
unavailable instead of displaying old results. Ordinary playback remains usable.

## Recommended first delivery

Reuse catalog and job infrastructure and update the tagger in place; keep Mood Library
review and the playlist workflow. Trial one additional audio model family and retain
only the evidence that improves decisions. Jev is an optional comparison on the same
evidence, reflecting the owner's ongoing investigation; it is not a release dependency.

| Component | First delivery | Expand only when |
|---|---|---|
| Evaluation | Small owner-judged, grouped pilot and untouched confirmation cohort | Results are inconclusive, more tags matter, or training needs more examples |
| Audio | Probe Discogs-EffNet with matching mood/theme and instrument heads; adopt only useful heads | A measured failure warrants another model |
| Native inference | Probe `tract-onnx` matching existing Tract; `ort` only if necessary | Compatibility and quality justify native packaging cost |
| Storage | Replace the generated-data contract in SQLite; add one bounded summary per track/analyzer if learned audio is adopted | An actual consumer needs retained tensors, history, or cross-file reuse |
| Interpretation | One new tagger contract with improved evidence; optional Jev comparison | Local training offers a measured quality, offline-use, or cost advantage |
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
- Replace the 30-track pilot format and fixtures with one grouped JSONL judgment/
  manifest format. Remove the old mode and parser. Start with roughly 60-100 representative
  recordings if available, including actual failures and a random library sample.
  This is a pilot size, not a statistical claim or a prerequisite for correcting defects.
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

**Gate:** capture baseline A once before replacement as a private research report;
the new runtime and pilot tools do not load old analysis formats. Freeze its configuration;
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
before production adoption. Verify the exact bytes imported by each worker, not only
a file hash observed at service startup. The voice path now enforces this with a
bounded owned snapshot; future model loaders must preserve that guarantee.
A failed probe records rejection or justifies one alternative runtime; it does not block useful
catalog/DSP corrections. Benchmark both heads, but ship only heads with a useful consumer.

## 3. Define the new storage contract and one-way reset

**Owners:** application `assistant` evidence types,
[storage](../crates/music-storage/src/analysis.rs),
[migrations](../crates/music-storage/src/migration.rs) and
[schema checks](../crates/music-storage/src/schema.rs).
Keep the eight-crate structure and current dependency direction.

| Record | New contract and cutover treatment |
|---|---|
| `track_contexts` | Clear old rows; recompute factual DSP/voice evidence from source audio |
| Proposed `track_audio_features` | One bounded record per `(track_id, analyzer_id)`: pipeline/source/model/preprocessing signatures, coverage/status, aggregate scores and selected intervals; no old-cache import |
| Catalog observations/result JSON | Preserve attributable source facts under current policy; refresh unsupported/expired cached payloads instead of adding readers for old formats |
| `track_analyses` | Replace the result shape with per-tag support/abstention and evidence references; clear old interpretations and remove obsolete columns |
| `track_user_tags` | Preserve accepted/operator-owned tags independently of generated analysis |
| `track_analysis_tag_reviews` and analysis failures | Clear state tied to superseded results; new proposals start a new review lifecycle |
| Private pilot files | One current manifest/judgment format; static pre-change research reports are not application inputs |

Use measured, catalog-observed and model-predicted input types. Missing, unavailable,
failed, partial and complete are explicit; missing is not zero. Per-tag decisions carry
support/abstention, bounded evidence references, contradictions and temporal scope.
Raw scores differ from optional demonstrated calibrated probabilities. Remove legacy
track-level confidence from analysis storage, DTOs and UI; do not carry a compatibility
summary or placeholder. Validate finite values, dimensions, references and payload limits.

Use forward SQLx migrations that resets derived records and removes obsolete
schema objects. Keep applied migration history intact; register the cutover migrations
with the matching writers/readers and old-path deletion in stage 9. It must preserve
source/authored data without translating old analysis payloads. Supersede old queued
analysis work before job recovery; completed/uncertain paid attempts remain audit-only
and are never resumed or reinterpreted as fresh results. Expire old analysis-role
certification/consent fingerprints and old review links under the new contract.

Use a proposed 64 KiB maximum learned-feature payload per track. Store summaries in
SQLite; retain full tensors only for selected private research fixtures. A separate
ledger/artifact store needs a concrete query or retention requirement.

**Gate:** fresh-database and upgrade/reset fixtures reach the same current schema;
reset preserves accepted tags and authored resources, rejects old result/job/review
identities, and recovers from interruption without exposing partial state. Repeated
startup preserves completed new-pipeline work; the reset migration runs once. Doctor and
backup/restore tests use the new contract. Restoring an older backup runs the same
one-way reset before analysis is available; there is no legacy-reader mode.

## 4. Connect existing sources and preserve correct invalidation

**Owners:** evidence projection;
[catalog workflow](../crates/music-application/src/cleanup_enrichment/workflow.rs),
[typed catalog port](../crates/music-application/src/cleanup_enrichment/catalog.rs),
[catalog invalidation](../crates/music-storage/src/catalog_evidence.rs).

- Include the new pipeline revision plus source/model/preprocessing/runtime identity
  in every generated result and job. Force fresh extraction for every track at cutover,
  regardless of matching old file signatures; subsequent reuse accepts only new-contract
  results. Bind decisions to allowed metadata, observation revision, source policy,
  vocabulary, engine and question/prompt revision.
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

**Gate:** rejection of pre-cutover results, source/model invalidation, ambiguous-match
exclusion, source-disable races, bounded raw-tag handling and duplicate-family grouping. A tag-only file edit
may trigger fresh analysis initially; measure that cost before adding another cache.

## 5. Correct misleading factual inputs and share required preprocessing

**Owners:** [context DSP](../crates/music-analysis/src/context.rs),
[voice streaming](../crates/music-analysis/src/voice.rs),
[shared mel frontend](../crates/music-analysis/src/musicnn.rs).

- The shared MusiCNN frame frontend is implemented behind pinned numerical fixtures.
  Keep it limited to the voice classifier and successful EffNet probe: 16 kHz, 512-sample frames, 256-sample hop, 96 Slaney mel bands
  and log compression. EffNet uses this feature family but **128-frame patches**,
  unlike the voice model's 187. Check centering, downmixing, resampling, silence and
  tails against the pinned reference. [EffNet preprocessing](https://essentia.upf.edu/reference/std_TensorflowPredictEffnetDiscogs.html).
- Voice decoding now matches the pinned MonoMixer's levels for mono/stereo and passes
  native-rate, 44.1/48 kHz count/level, ending, invalid-value and cancellation regressions.
  One final full patch covers the ending without repeating tail frames; summaries use
  constant storage. Decoder/window identities invalidate previous generated contexts.
  Full resampling spectral parity, multichannel behavior and the production resource
  gate remain separate from these basic checks.
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

- Run one bounded full-library rebuild through the existing durable job/executor
  infrastructure, defaulting to one worker. Recompute factual DSP and every enabled
  voice/learned-feature stage from original audio for every indexed track. Checkpoints contain
  only new-pipeline work; interruptions resume that rebuild without importing old results.
  Missing/failed files remain explicit. Cancellation never marks partial work complete.
  Model availability remains independent of boot/playback; release memory after the pass.
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
the full rebuild; reduce scheduling or reject the stack if the product budget is exceeded.

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
new evidence must not become hidden local preselection of allowed tags. Replace superseded
input/output parsers and fixtures. Version input, output, analyzer, disclosure, role
fingerprint and quality fixtures together. Do not
send evidence under the previous consent contract or register the old tagger as a fallback.

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

Land the reset migration together with current writers/readers, generated HTTP types,
strict browser guards and new fixtures. Remove superseded analyzer registrations,
legacy fields/parsers, compatibility branches and old pilot entrypoints. Old clients
must refresh/update rather than receive an adapted old analysis response. Current
provider alternatives such as Jev implement the same new contract.

**Gate:** stale-review races, bulk acceptance atomicity, partial-source disclosure,
judgment isolation and focused frontend tests. Verify old payloads/jobs/review requests
are rejected and no old analysis route remains reachable. Applied migration history stays
intact; static research reports stay outside the runtime. No playback wire change is planned;
inspect/update Baton only if a consumed schema changes. No additional frontend state store.

## 10. Confirm benefit, cut over once, and rebuild the whole library

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
workflow benefit; inconclusive results do not justify cutover or claims about all
138 labels. Drop models that add no value and retain independently useful fixes.

Validate the new pipeline on the pilot and representative folders before cutover.
Then stop old analysis workers, back up the database/authored files, deploy the contract/reset
change and supersede old analysis jobs before recovery. Rebuild every indexed recording
from original audio under the new contract, including previously successful tracks.
Recreate metadata/catalog-derived proposals from retained permitted source facts and
recompute local DSP plus every enabled voice, feature and interpretation stage. Refresh
source observations where policy/freshness requires it. No old result may satisfy the rebuild.

Use one resumable rebuild with visible pending/complete/failed counts; unavailable files
stay failed until accessible. New checkpoints can resume completed new-pipeline work.
Paid interpretation still requires the current disclosure and budget, with no automatic
retry of uncertain past attempts. Analysis failure leaves results unavailable; there is
no switch back to old analysis. Accepted tags change only through explicit review.

Verify reset/restart/restore, source withdrawal and preservation of authored data.
Measure concurrent playback under the container budget before the full rebuild. A model,
cache or provider failure must leave browsing, playback and authored playlists usable.
A later model/contract change invalidates affected generated results for reanalysis;
it does not add a reader for their old format.

Use logical local commits after focused checks; ship the destructive migration and
its matching runtime/UI changes as one coherent cutover. Runtime changes use
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
