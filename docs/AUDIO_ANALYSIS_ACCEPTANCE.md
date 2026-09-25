# Audio analysis acceptance probes

Use these read-only tools before a large context rebuild to check the real extractors
on representative recordings. The factual probe runs the same FFmpeg/RustFFT
implementation on one fixed analysis worker, without a model. The optional
[voice probe](#voice-repetition-and-lifecycle-probe) exercises the configured MusiCNN
worker separately. Neither starts a server, accesses the database, changes tags or
contacts providers.

This is extraction and operational evidence. Listening judgments remain in the
[grouped mood pilot](MOOD_PILOT.md). Model preprocessing/output parity and
whole-application resource acceptance are separate checks.

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
starts with `CONTEXT_PROBE_JSON `, followed by one `context-probe/v3` JSON record. The zero-based
`index` maps to the manifest and `iteration` distinguishes repeated passes.
No paths, filenames, embedded metadata or raw error messages are emitted.

Reports include:

- Analyzer/implementation identity, platform, audio duration and wall time.
- Time spent in each extractor stage, plus processing seconds per audio second
  (smaller is faster). Stage timings are observations, not quality scores.
- Decoded coverage, timeline/section counts and the last section's ending.
- Numeric `loudness`: `status: ebu_r128` carries `integrated_lufs`,
  `loudness_range_lu`, `true_peak_dbtp` and `relative_threshold_lufs`;
  `status: dbfs_proxy` carries only `rms_dbfs` and `peak_dbfs`.
  Unknown measurements are null. The report copies only these approved fields;
  it never copies arbitrary technical metadata. Error/cancelled records have no loudness.
- Process RSS before/after a recording and lifetime peak RSS where available.
- Optional cgroup v2 snapshots before/after each recording, using `--cgroup-dir`.
  See the [resource scope and interpretation](#resource-boundary-and-production-acceptance).
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

## Loudness reliability and optimization decision — 24 September 2026

The extractor still uses `loudnorm` input measurements with its original parameters.
FFmpeg describes this filter as a normalizer and notes its 192 kHz dynamic-mode
processing; the normalized output is discarded by our null sink. A measurement-only
scanner could avoid work, but it must pass numerical acceptance first.
See the [FFmpeg filter reference](https://ffmpeg.org/ffmpeg-filters.html#loudnorm).

A reproduced capture defect is fixed: an 80 kB multiline embedded comment generated
about 308 kB of stderr. The previous first-64-KiB buffer discarded the final report,
silently selecting `dbfs_proxy` for otherwise measurable audio. Capture now drains
both pipes, retaining only the last 64 KiB of stderr and the first 64 KiB of ffprobe
stdout. A real-FFmpeg regression compares a clean stereo fixture and a copy with long
notes: their four loudness measurements must match exactly. Other regressions cover
bounded capture, finite/complete JSON, mono/stereo levels, silence and a final-sample peak.

Codec and filter pools are explicitly set to one in both factual decode and loudness
passes. FFmpeg's filter pool has its own setting and otherwise defaults to available
CPUs; setting codec threads alone does not bound it.
See [FFmpeg advanced options](https://ffmpeg.org/ffmpeg.html#Advanced-options).
This is a pool limit, not a claim that the whole child process has only one thread,
nor a cgroup CPU/RAM acceptance result. Thread settings alone showed no material
speed improvement on the long synthetic PCM fixture.

`local-context/v3+rustfft/v2+loudness/v2` marks the revised capture/execution behavior.
Existing freshness rules require regeneration of earlier contexts; no migration,
legacy reader or automatic paid rerun is added. Accepted/manual tags remain authored state.
The probe now emits only its current v2 shape, with numeric loudness in a nested object.

### Release verification of the capture fix

The same unconstrained Windows host ran eight generated cases three times each through
the previous release probe and the rebuilt probe: 48 complete passes in total. The five
original PCM cases were joined by the multiline-comment case and FLAC/MP3 encodings of
the changing-ending fixture. All pairs retained matching coverage/section/timeline counts;
known PCM durations and section endings remained within 1 ms.

All 18 measured runs from the rebuilt probe matched all four numeric input measurements
from the original FFmpeg command exactly. Six silence/isolated-impulse runs retained the
explicit proxy because no finite integrated measurement was available. The metadata case
changed from proxy to valid EBU measurements in all three pairs. The ten-minute fixture
took 7.893–7.936 s before and 7.926–7.941 s after: no meaningful speedup is claimed.
Six cancellations requested after 100 or 500 ms completed with typed cancelled results;
the longest observed cleanup latency was 48.57 ms. No cancelled record contained
completed loudness or coverage.

The final batch passed 502 Rust tests, strict workspace Clippy, formatting, architecture,
workspace check, doc tests, generated contracts and the release probe build. These tests
protect extraction mechanics and current numerical behavior; they do not certify loudness
standards compliance, musical usefulness or production resource limits.

### Rejected direct scanner substitution

On the installed Windows FFmpeg 9.0.2, `ebur128=peak=true:framelog=verbose` was faster
but failed these comparisons. These observations are a reason to reject a drop-in
replacement, **not** a certification that either filter is the measurement reference.

| Generated signal | Existing loudnorm report | Direct ebur128 report |
|---|---|---|
| Impulse at the final sample, 16 kHz | True peak +0.70 dBTP | Peak -86.2 dBFS |
| Constant opposite-phase stereo tone, 3.25 s | LRA 0.00 LU | LRA 20.0 LU |
| Quiet ten-minute tone with a louder final four seconds | Integrated -19.09 LUFS | Integrated -20.8 LUFS |

An explicit 192 kHz resampler before the scanner retained the final impulse, but did
not resolve the other differences. Its default text summary also prints one decimal
place versus two in loudnorm JSON; display precision must not be confused with
measurement accuracy. The upstream implementations differ, so a future replacement
needs independent reference signals for integration/gating, loudness range and true peaks,
including short clips and endings. Relevant primary sources:
[scanner documentation](https://ffmpeg.org/ffmpeg-filters.html#ebur128),
[scanner source](https://ffmpeg.org/doxygen/trunk/f__ebur128_8c_source.html) and
[normalizer source](https://ffmpeg.org/doxygen/trunk/af__loudnorm_8c_source.html).

No replacement scanner, new audio library, alternate fallback algorithm or model was
added. Absolute loudness remains local technical evidence, outside mood-tagger input.
A separate loudness pass remains the measured bottleneck; its optimization is open,
with correctness acceptance required before adoption.

## Voice repetition and lifecycle probe

Build the existing optional voice probe and supply the separately licensed,
checksum-pinned model described in the [voice architecture](RUST_REWRITE_ARCHITECTURE.md#voice-inference):

```powershell
cargo build --locked --release -p music-analysis --bin music-voice-probe
Get-Content -Raw ./private-voice-paths.json |
  ./target/release/music-voice-probe --model ./voice_instrumental-musicnn-msd-2.pb --ffmpeg ffmpeg --repeat 3 |
  Set-Content ./private-voice-report.jsonl
```

Input limits match the factual probe: 4 MiB, 1-512 paths and 1-20 repetitions.
Readiness loads and releases a model once. Every pass then starts one fresh worker,
processes the tracks sequentially, and joins/releases the worker before the next
pass. The optional `--warmup` analyzes the first track once before each measured
pass; its time is reported separately. It does not provide a controlled cold start.

The current-only report is `voice-probe/v3`, prefixed with `VOICE_PROBE_JSON `.
Three record types share the verified source signature, platform and memory scope:

- `initialization`: readiness duration and memory before/after its temporary worker.
- `track`: zero-based manifest index and pass iteration, elapsed time, numeric
  `voice_score` / `vocal_coverage`, prediction-window count and memory before/after.
  Failed or cancellation-mode records carry no scores or window count.
- `pass`: track/failure counts, start/warmup/release timings, total pass time and
  memory before/after starting and joining the worker. `complete` means all requested
  track operations succeeded; in cancellation mode those operations must be cancelled.

Only approved numeric measurements and typed failure codes are projected. Paths,
embedded tags, arbitrary model summaries and raw decoder/IO errors are excluded.
A failed track does not prevent later inputs or passes. Initialization, worker-start,
warmup or output failures terminate the command with a nonzero exit; an incomplete
report cannot pass acceptance. Only the current v3 output is emitted.

Exercise cancellation with recordings long enough to remain in inference:

```powershell
Get-Content -Raw ./private-long-voice-paths.json |
  ./target/release/music-voice-probe --model ./voice_instrumental-musicnn-msd-2.pb --ffmpeg ffmpeg --repeat 3 --cancel-after-ms 100 |
  Set-Content ./private-voice-cancellation.jsonl
```

The timer starts at each analysis request, after worker initialization. Delays are
1-1,800,000 ms. Cancellation mode rejects `--warmup` so it cannot silently run an
entire uncancelled warmup before measuring. It requires both a cancellation request
and the typed cancelled result, awaiting inference and decoder cleanup before
proceeding. Finishing before or despite the request is `cancellation_not_observed`
and a nonzero exit. Repeat with different delays. A single Tract prediction cannot
be interrupted midway; this remains cooperative cancellation, not a hard watchdog.

`voice_score` is an uncalibrated model score; `vocal_coverage` is the fraction of
voice-leading overlapping windows, not measured vocal seconds. Window count alone
cannot establish decoded duration or musical correctness. Compare original duration
and factual coverage separately. Joining a worker proves its graph lifetime ended;
process RSS can remain above the initial level because of allocator/cache retention.
Use repeated pass observations to investigate growth before drawing conclusions.

## Original-audio acceptance — 25 September 2026

With owner permission, both release probes processed all 22 recordings from two
local albums: stereo AAC at 44.1 kHz, 207.40–310.03 seconds each, totaling
5,469.19 seconds (91.15 minutes). Each extractor ran two complete passes, one probe
at a time on Windows x64 GNU with Rust 1.97.1 and FFmpeg/ffprobe 9.0.2 (Gyan full
build). These were unconstrained local runs; disk caches were not controlled.
The manifest, per-file hashes and reports remain private ignored artifacts.

| Observation | Factual extractor | Optional voice worker |
|---|---|---|
| Completed track operations | 44/44 | 44/44 |
| Elapsed time per track | 2.930–4.337 s | 3.298–4.942 s |
| Processing seconds per audio minute | 0.835–0.870 | 0.943–1.085 |
| Repeat agreement | Duration, coverage, section ending and all four loudness fields matched exactly | Score, voice-leading window fraction and window count matched exactly |
| Additional planned cancellation checks | 12/12 cancelled | 12/12 cancelled |
| Longest observed cancellation cleanup | 35.26 ms | 24.32 ms |

All factual records retained `whole_track` decoded coverage and finite EBU input
measurements. The last section reached decoded duration within 0.44 ms; decoded
duration differed from ffprobe container duration by at most 21.66 ms. No proxy was
needed for this sample. Voice produced 139–208 prediction windows per recording;
the two model-owning worker starts took 17.06–18.50 ms and joins took 0.61–0.68 ms.
Readiness took 25.26 ms separately. These times exclude an application server and
do not predict production throughput or certify peak memory.

Cancellation used one original recording from each album, three repetitions each,
at both 100 ms and 500 ms. Every planned cancellation returned the typed result
after cleanup; none emitted completed scores, coverage or loudness. Separate CLI
controls verified missing-file failure with later-track recovery, malformed-input
rejection, repeated warmup, and nonzero exit when a four-second synthetic recording
completed before the cancellation timer. The last case emitted no voice score.

The voice source signature retained the pinned MusiCNN graph with
`tract-tensorflow/0.23.7+musicnn-compat/v1+preprocess/v1+decode/v2+windows/v2+artifact/v2`.
Factual extraction retained `local-context/v3+rustfft/v2+loudness/v2`.
The probe changes require neither a new analyzer identity nor a data migration.
Source SHA-256 hashes, sizes and modification times were unchanged after the runs.
No database, authored tags, provider calls or deployment participated.

This is the first operational acceptance sample on owner-supplied music. Two albums
do not establish broad format/domain performance. There are no independent listening
judgments or reference model outputs for these recordings, so repeatability cannot
certify vocal detection, mood accuracy or tabletop suitability. Windows RSS remained
null as designed; live Linux/cgroup memory, concurrent playback and the actual durable
production rebuild still need their own acceptance.

The batch passed all 519 Rust tests with the real pinned model and FFmpeg configured,
formatting, workspace check, strict Clippy, architecture, doc tests, generated
contracts and both release builds. Changed documentation links also passed.

## Durable rebuild recovery (25 September 2026)

The application job now avoids repeating completed work when the operator retries a
cancelled or failed forced rebuild. The previous code preserved successes on restart
of the same job, but an explicit retry has a new job ID and decoded completed tracks
again. The regression reproduced a completed context being overwritten by that retry.

A forced retry checks up to 64 compatible predecessor jobs. It reuses only parseable,
current-contract results with matching source identity and a job ID in that chain.
Prior results that the original forced rebuild never reached still run, as do failures
and changed source facts. Missing, incompatible or cyclic history cannot extend the
reuse set. Failed optional voice stages remain eligible while their current factual
context is retained; the follow-up below isolates their retry to the voice pass.
A newly requested forced job still recomputes every completed recording. Existing
partial audio checkpoints retain their voice-stage resume behavior.

Five regressions use the real context handler, bounded executor, durable coordinator
and SQLite, with a controlled synthetic analyzer so interruption points are repeatable:

- Two cancellations and retries preserve completed checkpoints from both predecessors,
  including their job attribution, while finishing work still required by the rebuild.
- Graceful shutdown, database close/reopen and coordinator restart preserve committed
  work and finish the same job on its second attempt.
- A missing file stays failed while other tracks finish; restoring it and starting
  ordinary analysis replaces the failure without repeating successful recordings.
- Changing indexed source facts after cancellation forces that recording to be
  analyzed again even though its earlier result belongs to the retry chain.
- A full context with an unavailable optional voice stage is attempted again on a
  forced retry, now without repeating its factual pass. With `MUSIC_TEST_VOICE_MODEL` and FFmpeg configured, the real pinned
  voice worker processes deliberately invalid synthetic inputs; decoder failures stay
  visible and never become successful classifications. The explicit model must load.

The tests check call counts, exact retained context rows, visible failures, preserved
manual tags and unchanged synthetic input files. The optional regression exercises
real worker loading and decoder failure, not successful inference or vocal accuracy.
Private-song analysis, abrupt process-kill recovery, concurrent playback and production
resource measurements remain separate from this durable-job acceptance.

The original forced-retry batch passed all 524 Rust tests with the pinned voice model and FFmpeg
configured, workspace check, strict workspace/fuzz Clippy, formatting, architecture,
doc tests and generated contracts. All 56 checked documentation links/anchors passed.

## Retry failed voice without repeating facts (25 September 2026)

A normal follow-up job previously treated a full factual context as finished even
when its optional voice stage was unavailable. A regression reproduced zero voice
attempts for two failed tracks. A forced retry could attempt voice again, but repeated
the completed factual pass first.

A new ordinary job now retries unavailable voice from earlier jobs while retaining
current factual measurements and successful voice classifications. Compatible forced
retries use the same boundary. Pending voice always resumes; an unavailable result
already saved by the same job is counted as failed without another attempt on restart.
This is one local attempt per requested job, with no background retry loop. Source,
implementation and model freshness checks still apply; a new forced rebuild still
recomputes completed factual profiles. No evidence identity or wire shape changed.

The voice-retry batch brought the recovery suite to seven cases. Two new tests cover a normal follow-up with
mixed successful/failed voice results and same-job restart after a recorded failure.
The forced-voice regression also checks that factual extraction is not repeated.
The tests use the real handler, coordinator, SQLite, pinned worker and FFmpeg, with
controlled factual extraction and deliberately invalid synthetic audio. Exact
retained factual fields, successful context rows, manual tags and input bytes are
checked. The successful voice row in the mixed fixture is seeded recovery state,
not a listening judgment; actual retry decoding failures remain visible.

The voice-retry batch passed all 526 Rust tests with the pinned model and FFmpeg, including
all seven recovery cases. Workspace check, workspace/fuzz Clippy, architecture,
formatting, doc tests, generated contracts and documentation links/anchors passed.
These checks do not establish vocal accuracy, production resource limits or playback
behavior, and no private originals or deployed library were changed.

## Analyzer panic recovery (25 September 2026)

A controlled analyzer panic reproduced permanent loss of the only analysis worker.
The first job failed as expected, but an explicit retry on the same coordinator also
failed because the pool could no longer execute extraction. A server restart was
previously needed to recreate its worker.

The fixed pool now catches a panic at the submitted-task boundary and returns
`TaskPanicked`. The job remains failed, with the fixed diagnostic
`context analysis task panicked`; panic payloads are not stored in the job error.
The same worker can execute subsequent tasks. Pool size, queue bounds, source
freshness, explicit retry and authored-state contracts are unchanged. No operation
is automatically repeated and no new runtime or dependency is added.

Two regressions cover this failure: three successive task panics preserve the same
worker thread identity, and the eighth SQLite-backed recovery case fails extraction
on the second recording then completes an explicit retry without restarting the
pool. That case retains the first recording's exact saved context, finishes the
remaining recordings, and preserves manual tags and source bytes. These fixtures
exercise Rust unwinding with controlled analysis; they do not reproduce a native
process crash, memory exhaustion or a production-library decoder defect.

All 528 Rust tests passed with the real pinned voice model and FFmpeg configured.
Workspace check, workspace/fuzz Clippy, formatting, architecture, doc tests,
generated contracts and documentation links/anchors also passed. Production
resource/playback and owner listening acceptance remain separate.

## Decoder deadlines and process exit (25 September 2026)

A subprocess fixture reproduced the missing factual decode deadline. The caller supplied
50 milliseconds, but the original stream reader waited until the fixture exited two seconds
later and returned a short-input error. Factual decode/frame accumulation now has a
30-minute budget, with checks during streaming and until FFmpeg exits. A timeout is a
failed extraction; the factual probe emits `error_code: timeout` without completed
coverage, duration or loudness measurements. Existing stream errors retain their cause
when cleanup terminates the decoder.

Both factual and voice paths now use a shared controlled exit wait. Audio EOF does not
prove that the decoder process exited. Cancellation and the current stage deadline remain
active during that wait; failure stops and reaps the child before joining its pipe readers.
The voice budget remains 30 minutes, including preprocessing and inference. The separate
ffprobe and loudnorm budgets remain 30 seconds and 30 minutes. No numerical algorithm,
model identity, dependency, storage contract or authored data changed.

Three subprocess regressions cover the stalled factual stream, deadline expiry during
process-exit waiting, and cancellation taking precedence over an already elapsed deadline.
Each verifies that the child is reaped. The probe regression verifies that a timeout never
claims completed coverage. The fixtures exercise the application's process control; they
do not certify Linux container behavior or interrupt an in-process Tract call midway.

All 532 Rust tests passed with the real pinned voice model and FFmpeg configured.
Workspace check, strict workspace/fuzz Clippy, formatting, architecture, doc tests,
generated contracts and all 150 local documentation links/anchors passed. Production
resource/playback, independent listening and the operator-started rebuild remain open.

## Cgroup observation tooling (25 September 2026)

Both probes now accept an explicit cgroup v2 directory and emit bounded, read-only
resource snapshots in their current v3 reports. Factual records include before/after
snapshots. Voice initialization and tracks have the same pair; pass records also
capture before/after worker start and release. No library, model, application service,
dependency or cgroup setting changes. There is no cgroup v1 fallback.

Five shared fixture tests cover counter/limit parsing, selected scope, read-only
behavior, missing/malformed/oversized data, and unsupported-platform handling; they
run in both probe binaries. The real-model voice regression verifies all nine records
across initialization, successful/failed tracks and two worker lifecycles. Windows
reports unsupported cgroup measurement explicitly. Three actual CLI smoke runs on a
four-second synthetic recording emitted five path-free v3 records: factual default,
factual with requested counters, and voice initialization/track/release. The default
reported `not_requested`; requested counters reported `unsupported_platform`.

All 542 Rust tests passed with the real pinned voice model and FFmpeg configured.
Workspace check, strict workspace/fuzz Clippy, formatting, architecture, doc tests,
generated contracts and all 150 local documentation links/anchors passed. No live
Linux/cgroup counter or concurrent-playback test ran: Docker/Podman and WSL are
unavailable on this host. Independent listening and production acceptance remain open.

## Resource boundary and production acceptance

Process counters come from `/proc/self/status`. Their scope remains **this probe
process only**: they exclude FFmpeg/ffprobe children, the server and other processes.
Peak RSS is process-lifetime, not per-recording. Unsupported or unreadable values
remain unknown.

For Linux cgroup v2 observations, add `--cgroup-dir <directory>` to either probe.
Select the scope explicitly and verify it contains the intended application and
its decoder children. The probes do not discover membership or substitute a host
cgroup automatically. For a container whose cgroup namespace root is the intended
application scope, a factual diagnostic can use:

```sh
./music-context-probe --ffmpeg ffmpeg --ffprobe ffprobe --repeat 3 \
  --cgroup-dir /sys/fs/cgroup < private-context-paths.json > private-context-report.jsonl
```

The operator supplies the probe binary separately; it is not added to the application
image. Preserve the chosen scope and environment details with the private report.
Paths never appear in the emitted JSON. Each counter file is limited to 4 KiB and a
v2 controller marker is required. Unsupported platforms, missing files, invalid
values and oversized data never become invented zeros.

| Observation | Meaning |
|---|---|
| `memory.current_bytes`, `memory.peak_bytes` | Current and lifetime peak memory for the selected cgroup and descendants. The probe never resets the peak. |
| `memory.max`, `cpu.max` | Local configured limits; explicit limited/unlimited/unavailable state. CPU quota and period are microseconds. These do not resolve ancestor restrictions or CPU affinity. |
| `cpu.usage_usec` | Cumulative CPU time for the selected cgroup and descendants. |
| `cpu.nr_periods`, `nr_throttled`, `throttled_usec` | Cumulative bandwidth counters for this cgroup's own CPU limit, not all ancestor throttling. |
| `memory_events` | Raw high/max/OOM/OOM-kill counters. Subtree event accounting depends on the mount's `memory_localevents` setting. |

These meanings follow the [Linux cgroup v2 interface](https://docs.kernel.org/admin-guide/cgroup-v2.html).
Compare before/after cumulative counters only while the selected cgroup persists;
a reset or recreation invalidates the difference. A lifetime peak is not a per-track
peak. Concurrent activity in the scope contributes to observations. Individual file
reads are not an atomic snapshot. `observed` means some counters were read, not that
all values are available or the production gate passed. Without the flag, snapshots
say `not_requested`; Windows says `unsupported_platform`.

Neither process nor isolated-probe observations alone pass the three-CPU/4 GB gate.
On the target container, measure the normal durable context job with ordinary
browsing and playback, including optional voice if enabled. Keep external container
monitoring for continuous sampling of that job; these probes only bracket their
own work. Record cancellation, restart/resume, failures, post-pass resource release
and playback behavior. Job checkpoints remain authoritative for the rebuild.

Independent listening and the operator-controlled production rebuild remain separate
acceptance steps. See the [implementation plan](SONG_EVIDENCE_IMPLEMENTATION_PLAN.md)
for delivered work, conditional models and the tool inventory.
