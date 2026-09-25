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
Local scripts, raw tensors and model artifacts remain ignored research output under
`target/`; this is a dated feasibility result, not a supported EffNet application
command or an additional production runtime.

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

The 24 September EffNet comparison establishes controlled frontend and same-export
ONNX graph parity. The 25 September voice checks establish basic mono/stereo decoding,
resampling counts, ending coverage and actual pinned-graph execution on this host.
Neither establishes original TensorFlow/ONNX encoder equivalence, full resampling
spectral parity, multichannel downmix parity, or usefulness on independently judged music.
Separate short/partial-hop constant-signal checks matched upstream frame counts and
centering; they did not exercise a complete EffNet wrapper.

Long-file repeated-run RSS/cancellation, production Linux container cost and concurrent
playback remain open. Cancellation/expiry is checked before and after each Tract call;
a wedged in-process inference call cannot be interrupted midway. No new model was
enabled, no production rebuild ran, and no owner listening labels were invented.
An EffNet candidate still needs development recordings, its own bounded extraction/tail
checks and the production resource gate before integration. Keep only the heads that
improve the owner's listening/session decisions.
