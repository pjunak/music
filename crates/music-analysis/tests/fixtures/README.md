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
revision and binary hashes identify this reference.

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

## Remaining acceptance

This establishes controlled numerical parity for one frontend build and the same
ONNX exports on two runtimes. It does not establish equivalence to the original
TensorFlow exports, correct decoding/downmixing/resampling, final-patch policy,
long-file memory/cancellation behavior, Linux container cost or usefulness on music.
Separate short/partial-hop constant-signal checks matched upstream frame counts and
centering; they did not exercise a complete model wrapper.

The ordinary voice framing and inference contracts remain unchanged. The optional
licensed voice-graph and FFmpeg-to-voice tests require operator-supplied weights;
they were unavailable for this batch and were not exercised. No new model was
enabled, no production rebuild ran, and no owner listening labels were invented.
A useful candidate still needs development recordings, a bounded extraction/tail
check and the production resource gate before integration. Keep only the heads that
improve the owner's listening/session decisions.
