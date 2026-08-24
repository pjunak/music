# ADR-008: Comprehensive local track context for mood tagging

**Status:** Accepted

**Date:** 2026-08-24

**Decider:** Project owner

**Implementation update (2026-08-25):** The input named below was v11 when this
decision landed. Follow-on voice evidence, grouped vocabulary, recall guidance, and
contract-recovery work advanced the current role to `assistant-music-tagger-input/v14`
and disclosure v10 without changing this ADR's local-context or privacy decision. See
the [living contract inventory](ASSISTANT_ARCHITECTURE.md#current-contract-inventory).

## Context

The previous library workflow had separate metadata and audio passes. Metadata analysis proposed
semantic tags before the model, while audio analysis reduced a recording to mostly global axes.
That created an inefficient create-tags-then-interpret-tags pipeline and lost important temporal
facts: a track with a quiet introduction and a late intense climax could look suitable for rest.

The library contains thousands of tracks and runs on a four-core, 8 GB server. Long local work is
acceptable, but raw audio, full spectrograms, and full-resolution timelines should not be sent to a
provider. Operators also need to inspect the evidence and choose what happens when only part of a
tagging scope has current context.

## Decision

- Replace the user-facing metadata/audio analysis sequence with one restartable
  `assistant.library-context-analysis` job using `local-context/v1`.
- Decode locally and derive factual whole-track trajectories for loudness, combined intensity,
  rhythmic drive, brightness, density, and spectral change. Also retain local tempo development,
  acoustic section boundaries and changes, repetition, EBU loudness when FFmpeg can provide it,
  technical media facts, analyzer confidence, and explicit per-stage status.
- Store the bounded summary, condensed timeline, major sections, technical facts, and stages in
  `track_contexts`, keyed by `(track_id, analyzer_id)`. Fingerprint path, size, and mtime. Commit or
  checkpoint failure after every track so interrupted work resumes without repeating completed
  decoding.
- Keep analysis semantic-free. It must not output terrain, scene, mood, genre, or instrument tags.
  Voice status remains `not_classified` unless the explicit optional local classifier described by
  ADR-009 is installed; spectral measurements must not be relabelled as voice detection.
- Give operators a Track Context tab mirroring library folders. The selected track view shows the
  timeline, trajectories, tempo/structure/voice status, acoustic sections, technical/stage details,
  and a playback control. The detail API omits the relative path because the browser selection
  already provides that context.
- Change mood-tagging input to `assistant-music-tagger-input/v11`. Send descriptive metadata,
  canonical library-relative path, the complete controlled-vocabulary definitions, and only a
  bounded projection of current local context. Do not send local candidate tag hypotheses, raw
  audio, waveforms, spectrograms, or full-resolution timelines. Do not ask the model to return
  energy/brightness/tension axes; those are local measurements, not model output.
- Constrain output to `assistant-music-tagger-output/v3`: exact track IDs, zero through eight exact
  vocabulary IDs, confidence, and at most four bounded evidence strings. Store validated names in
  `model-context-tagger/v6` and keep the existing explicit review/promotion transaction.
- Before every planned run, report full, partial, and missing/stale context coverage. The operator
  may either include incomplete tracks, using metadata/path alone where context is absent, or skip
  all tracks without full current context. Skipped tracks make no provider call and are reported in
  the durable job result.
- Invalidate model conformance and quality certification for the new role contract. Expand the
  synthetic suite to 43 cases, including quiet-intro/late-climax, slow-but-intense, fast-but-light,
  context-only non-invention, missing-context fallback, semantic ambiguity, and prompt-injection
  boundaries.

## Consequences

- Initial analysis takes longer and consumes more CPU, but it is paid once per unchanged track and
  yields reusable, auditable context for later AI pipelines.
- Mood tagging can reason about development rather than a single average. It still cannot infer a
  literal setting or scene from signal measurements alone.
- Context and semantic suggestions have separate storage and ownership. Deleting or rebuilding
  context cannot write mood tags, and accepting a model suggestion cannot alter audio-file metadata.
- Instrument recognition remains intentionally incomplete. ADR-009 adds the anticipated optional,
  versioned voice stage without presenting a signal heuristic as voice detection.
- Existing historical analysis endpoints may remain for compatibility, but the supported UI and
  new mood-tagging contract use only `local-context/v1`.

## Options considered

### Send spectrogram images to a vision model

Rejected for the default pipeline. It increases disclosure, cost, and latency while discarding
audio-specific timing detail. Local numerical condensation is more reproducible and auditable.

### Keep global BPM and energy only

Rejected. Averages cannot distinguish steady rest music from a track that builds into a climax.

### Let local analysis propose candidate tags

Rejected. It recreates the inefficient tag-generation/interpreting pipeline and can anchor the
model before it evaluates the full controlled vocabulary.

### Require full context before any tagging run

Rejected. It makes one failed or unsupported file block useful work. Explicit include/skip policy
keeps the trade-off visible and operator-controlled.
