# Factual audio acceptance probe

Use this read-only tool before a large context rebuild to check the real extractor
on representative recordings. It runs the same FFmpeg/RustFFT implementation on
one fixed analysis worker. It does not start a server, access the database, change
tags, contact providers or load a voice/mood model.

This is extraction and operational evidence. Listening judgments remain in the
[grouped mood pilot](MOOD_PILOT.md). Model preprocessing/output parity, voice
inference and whole-application resource acceptance are separate checks.

## Build and prepare

FFmpeg and ffprobe must be available. Build the release binary:

```powershell
cargo build --locked --release -p music-analysis --bin music-context-probe
```

On Windows without MSVC, use the GNU toolchain command described in
[Validation](VALIDATION.md). The binary is a developer/operator tool; it is not
added to the application image.

Create a private JSON array of absolute audio paths, in a fixed order. Keep the
manifest, audio hashes, FFmpeg/ffprobe versions and reports outside Git. Use a small
representative sample first: silence, very short audio, a partial final frame,
stereo, changing music and a long recording with a material ending. Synthetic
signals check mechanics; actual library recordings are still needed.

## Measure complete passes

```powershell
Get-Content -Raw ./private-context-paths.json |
  ./target/release/music-context-probe --ffmpeg ffmpeg --ffprobe ffprobe --repeat 3 |
  Set-Content ./private-context-report.jsonl
```

For a POSIX shell, redirect the same JSON file to standard input. Each stdout line
starts with `CONTEXT_PROBE_JSON `, followed by one JSON record. The zero-based
`index` maps to the manifest and `iteration` distinguishes repeated passes.
No paths, filenames, embedded metadata or raw error messages are emitted.

Reports include:

- Analyzer/implementation identity, platform, audio duration and wall time.
- Time spent in each extractor stage, plus processing seconds per audio second
  (smaller is faster). Stage timings are observations, not quality scores.
- Decoded coverage, timeline/section counts, the last section's ending and whether
  loudness used EBU R128 or the explicitly labeled proxy.
- Process RSS before/after a recording and lifetime peak RSS where available.
- Typed errors; failed/cancelled records never carry completed coverage.

Check decoded duration and the final section against the known reference duration.
A complete record shows what was decoded; it cannot prove a malformed file did not
omit audio or that the recording is musically suitable. An observed ending says
nothing about whether a model interpreted it correctly.

The first pass and later passes expose process/cache effects. Disk cache is not
controlled and this is not a cold-start benchmark. Limits are 4 MiB of input,
1-512 paths, 1-20 repetitions and one worker. Any extraction failure makes the
command exit nonzero after reporting the remaining inputs.

## Exercise cancellation explicitly

Use a recording long enough that analysis will still be running:

```powershell
Get-Content -Raw ./private-long-context-path.json |
  ./target/release/music-context-probe --ffmpeg ffmpeg --ffprobe ffprobe --cancel-after-ms 100 --repeat 3 |
  Set-Content ./private-context-cancellation.jsonl
```

The probe requests cancellation after the specified delay and awaits the extractor
and its owned subprocess cleanup. `cancellation_latency_seconds` starts at that
request; total elapsed time includes work before it. It then reuses the worker for
the next recording/pass. Try more than one delay to exercise different work stages.

Cancellation mode exits successfully only when every input returns the typed
`cancelled` result. A recording that finishes first is
`cancellation_not_observed` and produces a nonzero exit; it is not evidence that
cancellation passed. Missing files and decoder failures also fail the check.
The timer is cooperative, not an independent hard process-kill watchdog.

## Local smoke evidence — 24 September 2026

A Windows x64 GNU release build processed five generated PCM fixtures three times
each. This was an unconstrained local process with voice disabled, not the production
container or private library. All 15 passes retained duration and final-section
coverage within 1 ms of the fixture duration.

| Synthetic case | Audio duration | Observed elapsed range |
|---|---|---|
| Silence | 0.625 s | 0.149–0.182 s |
| Impulse in the final partial frame | 1.125 s | 0.147–0.153 s |
| Opposite-phase stereo, 16 kHz | 3.250 s | 0.201–0.207 s |
| Changing ending, stereo 48 kHz | 32.125 s | 0.617–0.623 s |
| Changing ending, stereo 44.1 kHz | 600.125 s | 8.037–8.076 s |

Six cancellation passes on the long fixture, requested after 100 or 500 ms,
returned the typed cancelled result. The longest observed cleanup latency was
48.81 ms. Separate CLI checks confirmed a nonzero exit for missing files, malformed
input and a short recording that completed before cancellation was requested.

The long fixture spent roughly 97% of its time in the existing separate FFmpeg
loudness pass. That identifies a concrete candidate for a later optimization; keep
its loudness/true-peak semantics and compare outputs before changing the implementation.
Do not infer a speedup on compressed recordings or production hardware from this
synthetic result. The extractor and its measurement contract are unchanged by this batch.

Windows process RSS is unavailable through this probe; the Linux counter parser
has unit coverage but was not exercised against a live Linux process here. No
container CPU/RAM budget, concurrent playback, voice/model parity or musical
accuracy claim follows from these results.

## Resource boundary and production acceptance

Linux counters come from `/proc/self/status`. Their scope is **this probe process
only**: they exclude FFmpeg/ffprobe children, the application server, playback and
other container processes. Peak RSS is cumulative for the entire probe lifetime,
not a per-recording peak. Windows and inaccessible/malformed counters return
`null`, never zero or a fabricated estimate.

Consequently these counters alone cannot pass the three-CPU/4 GB production gate.
On the target container, also measure total cgroup CPU/memory while the normal
durable context job runs concurrently with ordinary browsing and playback. Include
optional voice inference if enabled; use the existing `music-voice-probe` only
for isolated voice diagnostics. Record cancellation, restart/resume, failures,
post-pass resource release and playback behavior. Existing job checkpoints remain
the authority for the actual rebuild; this tool writes none.

Keep an independent listening comparison and the operator-controlled production
rebuild as separate acceptance steps. See the current
[implementation plan](SONG_EVIDENCE_IMPLEMENTATION_PLAN.md) for delivered work,
conditional models and the tool inventory.
