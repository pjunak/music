# ADR-012: Native context analysis and workload profiling

**Status:** Accepted

**Date:** 2026-08-25

**Deciders:** Project maintainer

## Context

ADR-010 lets independent tracks use separate processes, but aggregate CPU utilization alone cannot
show whether the remaining time belongs to decoding, spectrum calculation, feature summaries,
voice inference, EBU loudness measurement, or persistence. The `local-context/v1` spectrum path
also performed its 2,048-point FFT and per-frame sample arithmetic in Python. That preserved a
small dependency graph but spent substantial CPU time in interpreter loops inside every worker.

`local-context/v2` is reserved for corrected and recalibrated measurement semantics such as log-Mel
flux, tempo ambiguity, and improved structural segmentation. A performance implementation must not
claim that contract while it still produces the existing v1 evidence fields.

## Decision

Make NumPy a locked base dependency and use its native real FFT and array operations for spectrum,
frame, short-window, and accumulator math. Preserve the v1 Hann window, 1,024 output bins, band
boundaries, normalization, rounding, and stored/wire shapes. A deterministic two-tone calibration
fixture must remain numerically equivalent to the former implementation. The initial development
probe reduced 250 spectrum calculations from approximately 0.628 seconds to 0.053 seconds (about
11.8 times faster for that stage).

Keep the public analyzer ID as `local-context/v1`, but add the implementation identity
`local-context/v1+numpy-rfft/v1` to each source signature. Existing rows therefore become stale once
and rebuild through the normal durable job instead of being silently treated as current.

Measure every successful track with monotonic timers for:

- file probing;
- decode and frame metrics, excluding separately measured spectrum time;
- spectrum calculation;
- feature summaries;
- optional voice inference;
- EBU loudness;
- final document assembly.

Return only bounded aggregate timing in the durable job result: profiled-track count, wall time,
summed worker time, analyzed audio duration, audio-to-wall real-time factor, dominant stage, stage
seconds, and stage shares. Do not persist per-track timings or paths. Show the aggregate profile in
the authenticated Library context analysis UI.

## Options considered

### Keep the Python FFT and profile externally

This avoids a runtime dependency, but retains the known interpreter hotspot. Container CPU graphs
and sampling profilers also do not provide a durable, operator-readable breakdown for completed
jobs.

### Call the implementation `local-context/v2`

This would make staleness explicit but falsely imply corrected evidence semantics. Implementation
fingerprinting gives deterministic invalidation without consuming the semantic v2 contract.

### Replace the whole analyzer with Essentia or a GPU pipeline

This could consolidate native work, but would change algorithms, licensing, deployment support,
and calibration requirements. It remains a separate semantic analyzer decision.

## Trade-off analysis

NumPy increases the base installation and imports one native runtime in each analysis worker. The
production voice-enabled image already carried NumPy transitively, so production image growth is
limited; non-voice installations now pay that dependency cost. In exchange, the hottest repeated
numeric operations leave the Python interpreter while the established evidence contract remains
reviewable.

Embedded monotonic timings add small measurement overhead. They provide stable stage attribution
without requiring production `ptrace` permissions, but they are not a replacement for a sampling
profiler when investigating native code internals.

## Consequences

- The first post-deployment non-forced context check rebuilds every existing profile because its
  source signature lacks the new implementation identity.
- Job performance data remains operational metadata and is not sent to model providers.
- Failed tracks are counted in job failures but omitted from successful-stage timing totals.
- Stage totals are summed worker time and may exceed wall time when several workers overlap.
- `local-context/v2` remains available for a separately calibrated evidence redesign.

## Action items

1. Compare the next production rebuild's dominant stage, real-time factor, and peak memory with the
   pre-change run.
2. Use production profiling to decide whether the next optimization belongs in voice inference,
   decoding/loudness passes, or the semantic v2 analyzer.
