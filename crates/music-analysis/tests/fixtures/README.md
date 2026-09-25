# MusiCNN numerical reference

The [shared Rust frontend](../../src/musicnn.rs) independently implements the fixed
512-sample, 16 kHz, 96-band MusiCNN transform. The optional voice path uses it;
an isolated EffNet feasibility probe uses the same source. It owns no model runtime,
decoder, streaming policy or configurable feature framework.

## Offline regression fixture

[musicnn-reference-v1.json](musicnn-reference-v1.json) contains twelve synthetic frames
and 1,152 reference features. Cases cover silence, impulses at either boundary and
the center, DC, ordinary and quiet tones, near-Nyquist energy, two tones, a chirp,
clipping and deterministic noise. Exact float32 inputs are stored so host-language
signal generation does not affect ordinary Rust tests.

Expected values came from the upstream
[TensorflowInputMusiCNN API](https://mtg.github.io/essentia.js/docs/api/Essentia.html#TensorflowInputMusiCNN)
in `essentia.js@0.1.3`, package Git revision
`f46c91c08bdf263d5f3d575ab8fb0f9b81695acf`. The generator checks both the embedded
WASM and JavaScript API SHA-256 values before loading them. The upstream runtime
reports `2.1-beta6-dev`; that string is not an exact C++ source revision. The package
revision and binary hashes identify this reference. The JavaScript checksum field
uses the explicit name `core_artifact_sha256` to distinguish this public artifact digest
from an API credential. The generator still verifies both exact file hashes.

The fixed absolute tolerance is 0.0001 per feature. On Windows GNU, the maximum
observed error was 0.0000290871 before and after extraction into the shared module.
Tests reuse one frontend in forward and reverse case order, including silence after
nonzero frames. FFT scratch is allocated with the frontend and reused between frames.
The numerical definition and `preprocess/v1` identity are unchanged.

Normal CI consumes only the JSON fixture; it needs no reference package, network,
weights or audio files. Run the focused test from the repository root:

```powershell
cargo test --locked -p music-analysis preprocessing_matches_pinned_essentia_reference -- --nocapture
```

Follow [the validation guide](../../../../docs/VALIDATION.md) for the GNU toolchain
override on Windows. To independently regenerate and compare, explicitly install
the reference under ignored research output, outside application dependencies:

```powershell
npm install --prefix target/essentia-reference --ignore-scripts --no-audit --no-fund --save-exact essentia.js@0.1.3
node tools/musicnn-reference.mjs --essentia target/essentia-reference/node_modules/essentia.js --check crates/music-analysis/tests/fixtures/musicnn-reference-v1.json
```

The [generator](../../../../tools/musicnn-reference.mjs) can instead write
`--output target/musicnn-reference-new.json`. It refuses to overwrite an existing
file. Inspect changes before replacing the tracked fixture; never relax the numerical
tolerance to conceal a mismatch. Essentia.js is a separately licensed AGPL reference
tool, absent from application manifests and release dependencies. No upstream
implementation source was copied into the Rust frontend.

## Isolated EffNet comparison, 24 September 2026

The [implementation plan](../../../../docs/SONG_EVIDENCE_IMPLEMENTATION_PLAN.md#native-probe-evidence)
records the exact three ONNX artifact checksums. An ignored release-build probe
included the shared Rust module and ran Tract 0.23.7 at batch size one.
`onnxruntime-web@1.30.0` provided an independent CPU reference with one WASM thread,
using its documented [Node.js support and session API](https://onnxruntime.ai/docs/get-started/with-javascript/web.html).
Both references ran locally; no provider or private-library data was used. The ONNX
reference package artifacts were identified by SHA-256:

| ONNX Runtime Web 1.30.0 artifact | SHA-256 |
|---|---|
| `ort.node.min.js` | `f2ffa91920b249103bbfeb58a1a9b68bf92e9e9018bd164dc16da52bd14ee305` |
| `ort-wasm-simd-threaded.wasm` | `3398c10d07d229bd91b364548e130e0e51a8e5704b88c7c083ebbeb78842dee2` |

Five signals each contained 32,768 float32 mono samples at 16 kHz: silence, a quiet
440 Hz tone, 220/3,200 Hz mixed tones, a chirp with a level change, and deterministic
noise ending in a clipped square wave. The reference supplied the first 128 centered
frames through MusiCNN preprocessing, followed by the encoder and both matching
heads. The all-zero case used explicitly zero frames: the pinned JavaScript
FrameGenerator rejected an all-zero signal. This does not certify its silent-frame
policy or random-dither behavior.

Each case was checked twice: identical reference mel input into both graph runtimes,
then the Rust frontend and Tract graphs against the complete reference path.

| Check | Gate fixed before comparison | Observed worst case |
|---|---|---|
| Frame features, including 61,440 values in the five patches | Absolute error <= 0.0001 | 0.0000290871 |
| 1,280-value embedding | Cosine >= 0.999 on nonzero outputs | > 0.99999999999 |
| 56 mood/theme scores | Absolute error <= 0.001 | < 0.00000054 |
| 40 instrument scores | Absolute error <= 0.001 | < 0.00000090 |

These checks passed on this Windows GNU host. One observed combined graph load was
106 ms and individual encoder-plus-head calls were approximately 5-7.4 ms. Those
isolated timings are not a production throughput, cancellation or memory benchmark.
The original scripts, raw tensors and model artifacts remain ignored research output
under `target/`; the reference exporter below now makes selected-patch generation
repeatable. This is not an EffNet application feature or additional production runtime.

## Real-audio patch reference, 25 September 2026

The first real-audio attempt exposed a reference problem: Essentia.js 0.1.3's
[pinned FrameGenerator implementation](https://github.com/MTG/essentia.js/blob/f46c91c08bdf263d5f3d575ab8fb0f9b81695acf/src/cpp/includes/essentiajs.cpp#L70-L95)
deliberately skips silent frames. It dropped 1-59 frames in 20 of the 22 approved
recordings. Its frame index therefore cannot be used as an absolute timeline index.
The earlier constant/synthetic checks did not expose this real-file behavior.

The development-only [exporter](../../../../tools/effnet-reference.mjs) preserves
explicit centered 512-sample frames with 256-sample hops, including silence and
zero padding at either boundary. It selects the beginning, a middle patch on the
62-frame grid, and one full 128-frame patch anchored at the final centered frame.
Duplicate positions are omitted. It does not repeat a short tail or silently drop
one; input shorter than a complete patch fails. These are explicit probe semantics,
not a claim to reproduce the full upstream TensorFlow wrapper's final-patch policy.
See the upstream [frame semantics](https://essentia.upf.edu/reference/std_FrameCutter.html)
and [EffNet patch parameters](https://essentia.upf.edu/reference/std_TensorflowPredictEffnetDiscogs.html).

Prepare private float32-le, 16 kHz mono files from approved original audio. Record
the decoder version/command and source hashes alongside them; the exporter cannot
verify decoding or the original files from raw PCM. For the dated run, FFmpeg 9.0.2
decoded the same 22 stereo AAC recordings used by the acceptance probes, with:

```powershell
ffmpeg -v error -nostdin -n -filter_threads 1 -filter_complex_threads 1 -threads 1 -i original.m4a -map 0:a:0 -vn -af aresample=16000:out_chlayout=mono:rematrix_maxval=1 -ac 1 -ar 16000 -f f32le normalized.f32le
```

Keep a private JSON array of absolute PCM paths. Supply the explicitly installed
`essentia.js@0.1.3`, `onnxruntime-web@1.30.0` and the three separately licensed
ONNX artifacts pinned in the [plan](../../../../docs/SONG_EVIDENCE_IMPLEMENTATION_PLAN.md#native-probe-evidence).
These packages and model files are not application or CI dependencies:

```powershell
node tools/effnet-reference.mjs --inputs private-pcm-paths.json --essentia ./reference/node_modules/essentia.js --ort ./reference/node_modules/onnxruntime-web --models ./private-models --output private-effnet-reference.jsonl
node --test tools/effnet-reference.test.mjs
```

The tool also pins the WASM loader `ort-wasm-simd-threaded.mjs` to SHA-256
`e13f7f94fc51b4ca72b12faeb1ee95f4ace6dfbc8939bc718aabdc0a27c4299b` and loads
the verified Node entry directly. It validates exact model byte snapshots,
finite PCM and output dimensions. Limits are a 1 MiB input manifest, 1-32 recordings,
15 minutes per recording and three selected patches per recording. It processes
one recording at a time and one graph patch at a time. This bounds a development
experiment; it does not implement or certify a streaming production worker.

The current-only `effnet-patch-reference/v2` JSONL contains a provenance header,
patches with raw sample support, zero-based track/frame indices, valid sample
intervals, PCM hashes, feature tensors and graph outputs, then a completion record.
These private numerical exports contain audio samples: keep them outside Git.
Require a successful process exit, the completion record and its matching counts
before using a reference. Failed runs may leave incomplete output for inspection;
existing outputs are never overwritten. Errors omit input paths and tensor contents.
Raw scores are uncalibrated, and this exporter neither assigns library IDs nor
produces mood judgments or changes accepted tags.

The real run compared all 66 patches twice: reference features into Tract, then
shared Rust features into Tract, against the same pinned ONNX Runtime Web graphs.

| Check | Unchanged gate | Worst observed across both paths |
|---|---|---|
| 811,008 mel feature values | Absolute error <= 0.0001 | 0.0000290871 |
| 1,280-value embeddings | Cosine >= 0.999 | Minimum 0.999999999980 |
| 56 mood/theme scores | Absolute error <= 0.001 | 0.0000010133 |
| 40 instrument scores | Absolute error <= 0.001 | 0.0000011027 |

All 66 comparisons passed on Windows GNU. Ten dependency-free regression tests
cover framing, silent gaps, endings, duplicate patch removal, bounded input and
private failures; these run in CI. The existing 12-frame/1,152-value MusiCNN fixture
still reproduces exactly. Real-tool controls reject overwrite and altered weights;
a deliberately perturbed feature reference fails the numerical gate.
Source hashes, sizes and modification times, and all three model hashes, were
unchanged after the run. Private PCM, reference outputs and native probe remain
ignored research artifacts.

This establishes selected-patch feature and same-export graph parity on common
decoded music. It does not compare FFmpeg with an independent decoder/resampler,
prove original TensorFlow/ONNX export equivalence, run every patch of each track,
validate full streaming/aggregation/cancellation, measure production resource use,
or certify musical usefulness. The fixed explicit framing policy is tested
mechanically; the rejected convenience helper is not its temporal oracle.
Independent owner listening and the complete-path gates still determine adoption.

## Whole-track stream reference, 25 September 2026

The offline [stream exporter](../../../../tools/effnet-stream.mjs) uses the same
pinned local packages and exact owned model snapshots as the patch exporter.
It runs each centered frame once and one graph batch at a time. Its data buffers
retain 512 PCM samples, a 1,024-byte read buffer, 128 by 96 mel values and one
embedding/output set, independent of track length. Each inference disposes its
tensors; session cleanup is attempted on success, failure and cancellation.
It neither installs models nor changes the application dependency graph.

```powershell
node tools/effnet-stream.mjs --inputs private-pcm-paths.json --essentia ./reference/node_modules/essentia.js --ort ./reference/node_modules/onnxruntime-web --models ./private-models --output private-effnet-stream.jsonl
node --test tools/effnet-reference.test.mjs tools/effnet-stream.test.mjs
```

Limits remain 1-32 explicit PCM files, 16 kHz mono float32-le, at most 15 minutes
per recording and enough input for a complete 128-frame patch. Short inputs fail;
the tool does not fabricate repeated frames. Regular starts are 0, 62, 124, ...
frames, followed by one patch anchored at the final centered frame if it is not
already on that grid. Silent frames stay at their original positions. Only frame
boundary padding is zero-filled; no ending audio is dropped or repeated. This
explicit ending policy still differs from the upstream wrapper's discard/repeat
options and does not claim complete TensorFlow-wrapper parity.

For aggregation, each sample belongs to the nearest patch center: boundaries
are midpoints between adjacent centers, clipped to the real recording endpoints.
These non-overlapping weights sum to the decoded sample count and lie inside
their patch's valid support, including the off-grid ending. The versioned
`nearest_patch_center_time_partition/v1` convention produces weighted means and
population standard deviations; per-label maxima preserve brief high scores.
Scores remain uncalibrated. Weights are a summary convention, not independent
votes, identified mood changes or estimated mood duration. Quantiles, selected
interval projections and application storage still need an actual pilot consumer.

The current-only `effnet-stream-reference/v2` JSONL contains a provenance header,
ordered patch outputs with frame/support/weight positions, then each completed
track's PCM hash, coverage, counts and bounded summaries. It streams diagnostic
embeddings and head vectors to the private reference file, not an in-memory or
library-wide cache. It omits raw samples and mel tensors. Keep all audio-derived
outputs private and outside Git. A final completion marker appears only after
all tracks and runtime cleanup succeed; require it, matching counts and a
successful process exit. Cancellation emits a cancelled marker, returns CLI
exit 130 and never emits a completed summary for the partial track. Other
failures leave incomplete output. Existing output paths are always refused.

Checks detect truncated/growing PCM and changes to descriptor/path identity,
size or high-resolution timestamps. The PCM hash identifies exactly the bytes
consumed; these metadata checks do not make concurrent filesystem reads atomic.
Cooperative cancellation is checked while framing and around graph calls, with
an event-loop yield per patch so pending signals can run. A wedged synchronous
WASM inference call is not forcibly interruptible. Closing a session does not
prove that its allocator returns all memory to the operating system.

The same 22 approved recordings (91.1579 minutes) completed all 5,503 patches.
An isolated Rust comparator streamed the same PCM independently, used the shared
Rust mel frontend and Tract 0.23.7, verified the exact model byte snapshots, and
compared every patch's position, embedding and head outputs. Its independent
weighted sum/sum-of-squares/max calculation matched the incremental reference
summaries. All PCM hashes and coverage/counts agreed. A passing control and four
negative controls confirm that altered frame positions, changed head scores,
corrupted summaries and missing completion records cannot pass the native gate.

| Check | Fixed gate | Worst real-track observation |
|---|---|---|
| Patch embeddings | Cosine >= 0.999 | Minimum 0.999999999974 |
| Mood/theme outputs | Absolute error <= 0.001 | 0.0000025332 |
| Instrument outputs | Absolute error <= 0.001 | 0.0000019968 |
| Weighted means, standard deviations and maxima | Absolute error <= 0.001 | 0.0000013709 |

A separate 15-minute synthetic recording containing silence and a final
4,096-sample tone also passed all 907 patches and summary checks. Source audio
hashes, sizes and modification times, and the three model hashes, stayed unchanged.
Three actual-runtime cancellation controls returned no successful run or partial
track summary, including the CLI SIGINT handler exercised through Node's signal
event. This is cooperative handler evidence, not an OS forced-termination test.
The 66 earlier selected-patch references regenerated exactly after sharing the
runtime; the original 1,152-feature synthetic fixture also remained identical.

On this Windows host the reference pass took 184.93 seconds with a process peak
RSS of 226.75 MiB; the native comparator took 36.28 seconds including 126.7 ms of
graph loading, about 0.398 seconds per audio minute. The separate 15-minute
reference used 204.88 MiB peak process RSS. These are single development runs,
including private-reference I/O, not isolated inference benchmarks. They exclude
source decoding, the application server, playback and Linux cgroups; no production
CPU/RAM or memory-release acceptance follows from them. Native probe sources,
graphs and reports remain ignored research artifacts.

The whole-track batch passed 21 dependency-free tests covering framing, exact
sample partitioning, incremental/batch summary agreement, peaks, changed input
files, failures and cancellation, plus eight workflow-policy tests. No application
runtime, schema, provider projection or accepted/manual tag behavior changed.

## Original export pairing and label identity, 25 September 2026

The published [mood metadata](https://essentia.upf.edu/models/classification-heads/mtg_jamendo_moodtheme/mtg_jamendo_moodtheme-discogs-effnet-1.json)
and [instrument metadata](https://essentia.upf.edu/models/classification-heads/mtg_jamendo_instrument/mtg_jamendo_instrument-discogs-effnet-1.json)
name `discogs-effnet-bs64-1` as their encoder. The dynamic-batch ONNX encoder
has matching dimensions, but that alone cannot qualify an embedding/head pairing.
The original TensorFlow exports were therefore compared directly, independently
of Tract and the ONNX Runtime reference.

The three original graphs were downloaded from the official model catalog, with
no private upload. Their exact owned bytes were checked before GraphDef parsing:

| Original TensorFlow artifact | SHA-256 |
|---|---|
| `discogs-effnet-bs64-1.pb` | `3ed9af50d5367c0b9c795b294b00e7599e4943244f4cbd376869f3bfc87721b1` |
| `mtg_jamendo_moodtheme-discogs-effnet-1.pb` | `03f2b047020aee4ab39f8880da7bdae2a36d06a1508d656c6d424ad4d6de07a9` |
| `mtg_jamendo_instrument-discogs-effnet-1.pb` | `2e8c3003c722e098da371b6a1f7ad0ce62fac0dcfc09c7c7997d430941196c2a` |

The external reference used TensorFlow 2.8.0, matching the graphs' declared
framework version, CPython 3.10.21, NumPy 1.23.5 and protobuf 3.20.3. It used one
intra-op/inter-op CPU thread, disabled GPU and oneDNN, and imported each GraphDef
without rewriting operations. The hash-locked environment and Python helper
are outside this Rust-only repository and its release image; they are not a new
application runtime or supported Python subsystem. The official
[Windows TensorFlow wheel](https://pypi.org/project/tensorflow/2.8.0/#files)
has SHA-256 `b7170844ae6b048d82a9d7a61b2fa627f2e16cb829267bf0ce4b3a0de0a61054`.
The installed native binding hash is
`00a466654a00e03ca0c7e6bc515cf8578fa80fab870dfe96f61a8555d6c803ae`.

Reproduction contract: feed common float32 mel tensors into
`serving_default_melspectrogram:0` and read `PartitionedCall:1` from the
original encoder, shape `[64,128,96] -> [64,1280]`. Fill unused batch slots
with zero mel values and discard only their outputs. Each original head takes
`model/Placeholder:0`, shape `[n,1280]`, and returns `model/Sigmoid:0`
with 56 or 40 scores. These TensorFlow names differ from the actual ONNX names.
Run each head both on TensorFlow-produced embeddings and on the identical
reference ONNX embedding. Hash the float32 input bytes to prevent case mix-ups;
require every case, finite outputs and successful session cleanup before
writing a completed report.

All five synthetic and 66 real selected patches passed the unchanged gates:

| Comparison with the pinned ONNX stack | Gate | Worst observation |
|---|---|---|
| Encoder embedding cosine | >= 0.999 | Minimum 0.999999999986 |
| Original encoder + mood head | Absolute error <= 0.001 | 0.0000010133 |
| Original encoder + instrument head | Absolute error <= 0.001 | 0.0000007749 |
| Mood head, identical embedding input | Absolute error <= 0.001 | 0.0000001789 |
| Instrument head, identical embedding input | Absolute error <= 0.001 | 0.0000001491 |

Eight selected patches were moved to different batch positions with changed
neighbours and zero padding; embedding values were exactly identical.
Permuted embedding coordinates and altered expected head scores each failed
their negative-control run and produced no completed report. This qualifies
the named graph pairing on the tested inputs, not mood usefulness or full
decoder/resampler/upstream-wrapper equivalence. No source audio or ONNX weights
changed. Private inputs, reports and the external helper remain outside Git.

Both Node exporters now require matching `.json` metadata beside their ONNX
graphs. They verify these exact files before loading a model runtime:

| Metadata file | SHA-256 |
|---|---|
| `discogs-effnet-bsdynamic-1.json` | `a2e85b2e7372d5f8e0f35bdd6aeae1139f101087d183d0b2fb60b0ea0f01a0ff` |
| `mtg_jamendo_moodtheme-discogs-effnet-1.json` | `d62cd90263e4d613fa7fcce7a831e339450394794af63685f96e065c1a896ab0` |
| `mtg_jamendo_instrument-discogs-effnet-1.json` | `7d02204c6451b5615e2968ec6364bbae3b915c886e608f05f00d3a38dc5177c4` |

Current patch/stream formats are v2. Header `artifacts.mood.metadata.labels`
and `artifacts.instrument.metadata.labels` bind vector positions to the
exact published 56/40 label order. Names, versions, sample rate, dimensions,
unique nonempty labels and the documented encoder pairing are checked.
No sorting, aliases or owner-vocabulary mapping is performed. Metadata includes
its file/hash provenance; the unused 400 encoder style labels are not exported.
Changed label order fails before runtime loading. Regenerate older private
references to obtain this provenance; no compatibility mode was introduced.

The 26 dependency-free reference tests pass. Current v2 generation reproduces
all 66 selected patches and one full 226-patch track with its summaries exactly;
that track also passes the updated native comparator. A real-tool control rejects
a reversed metadata label array. The older full-corpus numerical result remains
separate from this metadata-only repeat; no application model, provider projection,
database, accepted/manual tag or production resource contract changed.

## Voice decoding and ending acceptance, 25 September 2026

The optional voice path now normalizes FFmpeg's stereo-to-mono matrix before
resampling to 16 kHz. The existing pinned Essentia.js MonoMixer independently
returned 0.25 for identical 0.25 channels, zero for opposite-phase 0.25 channels,
and 0.25 for a 0.5 left channel plus a silent right channel. FFmpeg's previous default
floating-point downmix returned approximately 0.35355 for the first case. The
`aresample=16000:out_chlayout=mono:rematrix_maxval=1` filter restores the expected
level. Both codec and filter pools are explicitly bounded. See the upstream
[MonoLoader contract](https://essentia.upf.edu/reference/std_MonoLoader.html) and
[FFmpeg resampler options](https://ffmpeg.org/ffmpeg-resampler.html).

Regression tests exercised FFmpeg 9.0.2 on Windows GNU:

- Native 16 kHz mono PCM preserves sample levels and the final centered frame.
  A four-second recording with sound only in its final second reaches two windows;
  previously only the first, silent window reached inference.
- Four-second 44.1/48 kHz in-phase stereo signals produce 251 frames and two windows,
  with interior mel values within 0.0001 of the arithmetic-mean reference.
  Opposite-phase stereo cancels to zero. These are basic level/count/resampling checks.
- Short signals and lengths around patch boundaries verify complete windows, bounded
  buffering and no duplicate aligned ending. Invalid PCM or model values fail the
  entire result. Cancellation during prediction and expiry before the ending cannot
  publish a completed summary; FFmpeg cleanup returned within five seconds locally.

The regular voice patch remains 187 frames with a 93-frame hop. One additional full
patch uses the actual final 187 retained frames when the regular grid misses the
ending; it does not repeat the last partial patch. This deliberate policy differs
from upstream [discard/repeat options](https://essentia.upf.edu/reference/std_TensorflowPredictMusiCNN.html).
Very short audio without 187 centered frames remains unavailable. Summaries retain
only scalar counts and totals, plus a reusable patch buffer; no prediction list grows
with recording length. The mean normalized score and voice-leading window fraction
remain uncalibrated window statistics. Overlapping windows are not independent
observations or measurements of vocal seconds.

The existing [official model metadata](https://essentia.upf.edu/models/classifiers/voice_instrumental/voice_instrumental-musicnn-msd-2.json)
identifies `voice_instrumental-musicnn-msd-2.pb`, input `[1,187,96]` and
instrumental/voice output order. The downloaded graph's SHA-256 matched the existing
pin: `b734bca3fc99257cf0088211b44bd36e8a26fbb1f9ce67e1e97d39f188094b0a`.
The fixed zero-input output matched `[0.378066, 0.33894423]` within 0.0001.
The real FFmpeg-to-worker test classified both windows, handled an already-cancelled
request without killing the worker, and shut down within five seconds. Weights remain
ignored developer artifacts, separately licensed and never installed by the application.

The source identity is now
`tract-tensorflow/0.23.7+musicnn-compat/v1+preprocess/v1+decode/v2+windows/v2+artifact/v2`.
Older generated voice contexts become stale through the existing identity check.
No legacy reader or data migration is needed; accepted/manual tags are preserved.

To exercise these checks with an explicitly installed model and FFmpeg:

```powershell
$env:MUSIC_TEST_VOICE_MODEL = 'path/to/voice_instrumental-musicnn-msd-2.pb'
$env:MUSIC_TEST_FFMPEG = 'path/to/ffmpeg'
cargo test --locked -p music-analysis --lib -- --nocapture
```

Decoder regressions can use FFmpeg from PATH without model weights; graph/worker
checks need both explicit variables. An invalid supplied executable or model fails
the check rather than silently skipping it.

## Model integrity follow-up, 25 September 2026

A regression modified the official graph's output-layer bias in a temporary copy
after startup validation. The previous worker factory accepted that graph under
the original pinned identity. Every worker now reads a bounded byte snapshot,
verifies its SHA-256 and parses that same owned snapshot before graph adaptation.
The parser no longer reopens or memory-maps the configured path after verification.

Model input is capped at 4 MiB; at most one additional byte is read to detect
overflow. Startup identity checks use the same bounded reader. Tests reject an
endless input and preserve read errors, reject an unverified graph before import,
reject replaced/deleted models at worker start, and recover after the exact model
is restored. The replacement/recovery check exercised the real licensed graph.
Golden zero-input outputs and FFmpeg worker inference still pass.

The added `+artifact/v2` identity makes results from the previous loading policy
stale. No new model or compatibility reader is introduced. This establishes
artifact attribution and bounded input handling, not mood accuracy or a production
memory/cancellation benchmark.

## Remaining acceptance

The EffNet comparisons establish synthetic/selected-patch feature parity and
complete-track same-export ONNX parity with time-weighted summaries on common PCM.
The 25 September voice checks establish basic
mono/stereo decoding,
resampling counts, ending coverage and actual pinned-graph execution on this host.
The separate 71-patch original-export comparison now qualifies the named
TensorFlow/ONNX graph pairing. Full resampling spectral parity, multichannel
downmix parity and usefulness on independently judged music remain open.
Separate short/partial-hop constant-signal checks matched upstream frame counts and
centering; they did not exercise a complete EffNet wrapper.

The voice acceptance probes now cover repeated real-file runs and cooperative
cancellation on Windows. Live Linux RSS/cgroup memory and concurrent playback remain
open. The EffNet reference supports bounded streaming and cooperative cancellation;
its native production worker and memory-release lifecycle are not implemented or qualified.
Voice cancellation/expiry is checked before and after each Tract call;
a wedged in-process inference call cannot be interrupted midway. No new model was
enabled, no production rebuild ran, and no owner listening labels were invented.
An EffNet candidate still needs independently judged development recordings,
production lifecycle/resource gates
before integration. Keep only the heads that
improve the owner's listening/session decisions.

## Decoder spectral checks, 25 September 2026

Two real-FFmpeg regressions now exercise the complete decoder-to-voice-feature path
at 44.1 and 48 kHz. The four-second PCM fixtures are generated during the tests; no
recordings, model weights or new reference dependency are required.

- Distinct 1 kHz left and 4 kHz right tones are compared with an analytic arithmetic-
  mean signal at 16 kHz, transformed by the already fixture-verified mel frontend.
  Three interior frames check all 96 bands. The gate is 0.02 absolute log10-feature
  units, approximately 0.2 dB in `1 + 10000 * mel power`; this does not alter the
  separate 0.0001 transform-reference tolerance. Maximum observed error was
  0.00003485 at 44.1 kHz and 0.00001574 at 48 kHz.
- 9 and 12 kHz mono tones must be suppressed before they can alias into the model's
  lower bands. The gate is at least 60 dB attenuation of maximum interior mel power
  relative to an unfiltered aliased tone of the same amplitude. Log compression is
  inverted before computing the ratio; an absolute compressed-feature value is not
  a decibel measurement. Worst observed attenuation was 61.47 dB; all four cases
  ranged from 61.47 to 103.50 dB.

All six input cases retained 251 centered frames and two prediction windows. A
controlled run temporarily used `filter_size=1:phase_shift=0:linear_interp=0`:
frame/window counts still passed, but the spectral gate failed at only 3.06 dB
attenuation. The exact production filter was then restored. These tests add a guard
against spectral damage that level/count tests alone could miss.

The pinned Essentia.js package exposed a `Resample` wrapper but threw during valid
synthetic conversion on this host. Follow-up diagnosis found no registered
`Resample` algorithm in this exact build: its caught exception says
`Identifier 'Resample' not found in registry...`, and its exposed algorithm-name
list also omits it. No full reference-resampler comparison was obtained.
Upstream [Essentia Resample](https://essentia.upf.edu/reference/std_Resample.html)
uses SRC, while [FFmpeg's default resampler](https://ffmpeg.org/ffmpeg-resampler.html)
is SWR. Analytic passband/stopband checks do not establish equivalence between those
filters, their transition bands or edge policies. Full upstream resampler parity,
multichannel acceptance, Linux production resources and listening quality remain open.
No production decoder setting, numerical identity or model score semantics changed.
