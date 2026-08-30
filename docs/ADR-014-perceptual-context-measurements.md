# ADR-014: Perceptual local-context measurements

**Status:** Accepted

**Date:** 2026-08-26

**Deciders:** Project maintainer

## Context

`local-context/v1` produced bounded, reviewable evidence, but controlled signals exposed semantic
failures that made several percentages misleading. Brightness divided spectral centroid by at
least 6 kHz, compressing ordinary bright music toward zero. Three broad frequency bands could not
detect a 500 Hz to 1.5 kHz transition. The rhythm proxy changed with gain, an accented 120 BPM
pulse was reported as 60 BPM with complete confidence, and `high_fraction` was defined by each
track's own 75th percentile so it stayed near 25% regardless of absolute level.

These are meaning changes, not implementation-only optimizations. Reusing the v1 ID would mix
incompatible evidence in storage and in the mood-tagging provider contract.

## Decision

Introduce `local-context/v2` and make every v1 row non-current until the operator rebuilds it. The
mood-tagging input advances to `assistant-music-tagger-input/v17`; existing conformance and quality
records therefore become stale before v2 evidence can be sent to a provider.

Input v18 later removes titles and library paths without changing this v2 context projection; see
[ADR-006](ADR-006-review-only-model-music-tagging.md).

V2 keeps the bounded public field names needed by existing consumers, with these definitions:

- **Signal level (`loudness`)** maps each 0.5-second RMS level from -50 to -10 dBFS. Integrated
  programme loudness, loudness range, and true peak remain separate EBU R128 technical facts.
- **Brightness** combines magnitude-weighted spectral centroid on a logarithmic 250 Hz to 4 kHz
  scale with 85% spectral rolloff on a logarithmic 1 kHz to 7 kHz scale.
- **Spectral change (`spectral_flux`)** is the distance between gain-normalized, log-compressed
  24-band Mel spectral profiles, rather than change across three coarse bands.
- **Spectral fullness (`density`)** combines band entropy, occupied-band coverage, bandwidth, and
  flatness. Loudness is excluded, so gain alone cannot make a spectrum look denser.
- **Rhythmic drive** uses positive short-window dB rises above a robust local threshold. It is
  independent of absolute gain and no longer treats arbitrary spectral change as rhythm.
- **Intensity** remains an explicitly heuristic composite of signal level, rhythmic drive, and
  spectral fullness.
- **Tempo** keeps bounded autocorrelation but applies a broad musical-tempo prior to obvious octave
  alternatives and reduces confidence when a half/double-tempo rival is strong.
- **High fraction** is the fraction of timeline windows at or above two-thirds of the bounded scale,
  not a per-track percentile.
- **Major sections** use change evidence across multiple time scales before applying the existing
  minimum-section and maximum-section bounds.

Every heuristic axis is marked medium reliability in the stored summary. Tempo and structure fall
to low when unresolved or too short. Voice remains a separate checksum-pinned classifier stage and
is high only when that classifier completed; no spectral axis is relabelled as voice detection.

The Track context UI uses the more honest labels “Signal level” and “Spectral fullness.” It keeps
percentages as bounded evidence, not calibrated probabilities.

## Options considered

### Patch brightness while retaining v1

Rejected. It would fix the most visible symptom while preserving known tempo, rhythm, spectral
change, and trajectory defects, and it would silently change stored v1 semantics.

### Calibrate every axis from this one music library

Rejected. Corpus percentiles would make scores relative to the current collection, drift whenever
the library changes, and prevent meaningful comparisons across deployments. V2 instead uses fixed,
documented transforms and controlled-signal regression tests.

### Require a larger audio-feature or machine-learning runtime

Deferred. Essentia offers stronger beat tracking and many spectral descriptors, but the base
analyzer must remain lightweight and available without the separately licensed optional voice
runtime. NumPy plus the existing FFmpeg boundary is sufficient for this correction.

## Consequences

- The first v2 run must rebuild the library; v1 rows remain historical and are not current input.
- Brightness and spectral-change values will move materially, and model-tagging quality must be
  rerun before live provider jobs.
- Tempo remains an estimate with octave ambiguity, now reflected in confidence rather than hidden.
- The axes remain factual signal summaries, not semantic mood, genre, instrumentation, or calibrated
  perceptual truth.

## Action items

1. Rebuild the production library after deploying v2.
2. Review representative dark, bright, sparse, dense, steady, syncopated, slow, and fast tracks.
3. Rerun the complete mood-tagging conformance and quality suite before using provider tagging.
