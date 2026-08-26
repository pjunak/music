# ADR-009: Opt-in local voice analysis for track context

**Status:** Accepted

**Date:** 2026-08-24

**Deciders:** Project maintainer

**Implementation update (2026-08-25):** The voice stage initially advanced tagging to
input v12 and disclosure v8. Follow-on grouped-vocabulary and contract-recovery changes
advanced the current role to `assistant-music-tagger-input/v14` and disclosure v10. The
classifier identity and local-only boundary in this ADR are unchanged; see the
[living contract inventory](ASSISTANT_ARCHITECTURE.md#current-contract-inventory).

**Deployment update (2026-08-25):** The project-published production image now explicitly builds
the optional Essentia runtime. The model remains a separate operator-obtained, checksum-verified,
read-only mount; generic Docker builds keep the runtime disabled by default. Stored
`local-context/v1` rows retain the legacy `voice_probability` key for compatibility, but every
consumer treats the value as a normalized classifier score, not a calibrated probability.

## Context

`local-context/v1` deliberately reported voice as unknown because loudness, spectral shape, and
pitch-like signal features are not reliable evidence that a recording contains human voice. The
Track Context workflow now needs factual voice/instrumental evidence, a whole-track score, and an
estimate of how much of the recording is voice-leading. Audio must remain local and a failed optional
classifier must not discard otherwise useful context.

## Decision

Add `essentia-musicnn-voice/v1` as an optional stage inside the existing context job. It accepts only
the checksum-pinned Essentia `voice_instrumental-musicnn-msd-2.pb` model, whose declared output
classes are `instrumental` and `voice`. The stage normalizes each two-class window, stores the mean
voice score and the fraction of voice-leading windows, and presents a conservative track-level
description. The raw audio, windows, embeddings, and model file never leave the server.

The classifier is not automatically installed, downloaded, or enabled. The Essentia TensorFlow
runtime is AGPL-3.0-only and the MTG model weights are CC BY-NC-SA 4.0, so an operator must explicitly
build the `voice` extra, obtain the model under its license, verify the pinned SHA-256, mount it
read-only, and set `ASSISTANT_VOICE_MODEL_PATH`. The base application continues to report
`not_classified`.

Runtime readiness and source-signature checks execute the Essentia import in a short-lived Python
process. The parent FastAPI process does not perform inference and must not retain its own
TensorFlow copy merely to render deployment status. Analysis workers import and cache the predictor
only when they process a configured track.

The path-free classifier/model/runtime identity is part of each context source signature. Enabling,
disabling, installing, removing, or changing the model makes affected rows stale and requires the
normal context rebuild. A model or runtime failure produces an optional `unavailable` voice stage;
the remaining context stays complete. Because classified voice evidence may be included in the
bounded mood-model payload, the disclosure advances to v8 and the tagging role contract advances to
input v12, invalidating earlier consent and quality/conformance results.

## Options considered

### Google YAMNet under LiteRT

YAMNet is compact and Apache-licensed, but its official model card says scores are not calibrated
across classes and require task-specific calibration. Aggregating its singing/speech classes into the
existing `voice_probability` field would overstate what the output establishes.

### Signal heuristics

Rejected. Spectral energy, harmonicity, and pitch can describe many instruments and must not be
relabeled as human-voice detection.

### Source separation

Rejected for this stage. Separating a vocal stem is materially heavier in CPU, memory, model size,
and processing time, while stem leakage still needs a classifier or calibrated decision rule.

## Consequences

- Default installs remain lightweight, local, and honest about unknown voice status.
- Opted-in Linux x86-64 deployments gain time-aware, reviewable voice evidence without provider I/O.
- Windows development can exercise the contract and tests, but real inference needs the supported
  Linux runtime or a future independently validated backend.
- The published MTG accuracy is useful research evidence, not validation on the operator's own music;
  representative listening checks remain required before relying on the result.
