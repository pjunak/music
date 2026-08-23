# ADR-006: Review-only model evidence tagging

**Status:** Accepted

**Date:** 2026-08-19
**Decider:** Project owner

**Implementation update (2026-08-23):** Tagging is now scope-aware, may use the
canonical library-relative path as bounded untrusted evidence, and is launched from a
playback-capable Library review dialog. The accepted tag store remains database-only.

## Context

Manual D&D playlist tags are operator-owned, while local metadata analysis
already stores generated suggestions separately and requires explicit review.
An optional model can infer useful setting, scene, and mood labels from richer
metadata, but arbitrary tags would fragment the vocabulary and direct promotion
would erase the distinction between model output and human intent.

Library-wide model tagging can also take many paid requests and must remain
observable after the browser closes without silently repeating uncertain calls.

## Decision

- Reuse `track_analyses` and `track_analysis_tag_reviews` with the versioned
  analyzer ID `model-evidence-tagger/v4`. Do not create a parallel AI-tag store.
- Limit model input to numeric track ID, indexed title, display title, artist,
  album, origin, genre, canonical library-relative path, duration, BPM, and an optional bounded projection of a
  current `local-audio/v1` profile: energy, brightness, tension, tempo estimate,
  activity, normalized dynamic range, rhythmic density, rhythmic stability, and
  confidence. Also derive a `local-metadata-evidence/v1` hypothesis from the same
  disclosed descriptive fields and relative path, and send its bounded candidate tag IDs, the matched
  field and term for each candidate, canonical-title source, axes, and confidence. A
  non-empty display title is canonical for deterministic title matching. Treat every
  relative path and metadata string as untrusted data. Do not send the absolute media
  root, paths outside the indexed library, audio, waveforms, detailed signal measurements,
  database mood tags, stored local generated tags, playlists, or review history.
  Numeric signal evidence may refine generic mood and activity judgments but is
  never proof of an instrument, genre, setting, scene, or D&D context.
- Send at most 20 tracks per provider request. Treat every metadata string as
  untrusted prompt data and require one output profile for every input ID.
- Resolve scope before provider work. A run may target the whole library, a folder
  with explicit recursive or direct-child behavior, or an explicit set of track IDs.
- Store one revisioned operator-managed vocabulary with stable IDs, normalized names,
  selection definitions, groups, exact cleanup aliases, and overlapping local context
  cues. Send the full bounded ID/name/group index with each metadata batch, but send
  detailed definitions and exact aliases only for locally highlighted candidates.
  Context cues remain local and never act as cleanup mappings. Restrict output to zero through
  eight IDs from the current vocabulary plus bounded energy/brightness/tension values,
  confidence, and short evidence. Inject the exact track and tag ID enums into the
  provider schema, then resolve validated IDs to names locally. Deterministically retain
  only the first four bounded, well-typed evidence strings because they are incidental
  review text. Reject unknown tags, invalid axes or confidence, malformed core output,
  missing IDs, duplicate IDs, extra fields, and truncated responses.
- Require the exact current `music-tagging-quality-v1` certification and a
  versioned disclosure confirmation before enqueueing live work. Recheck the
  role fingerprint and quality gate around every provider batch and database
  commit.
- Bind each profile source signature to the consumed track metadata, optional
  local-audio source signature, vocabulary fingerprint, input-contract version, and
  exact model-role runtime fingerprint. Changed metadata, audio evidence, vocabulary,
  or model settings make old suggestions stale rather than silently current. Adding a
  first current audio profile also invalidates a metadata-only model result.
- Run the library pass as a durable, non-restartable job. Commit completed
  batches, skip unchanged profiles, and do not automatically repeat a provider
  call after a server restart. A deliberate later run safely skips committed
  current profiles.
- Keep model profiles out of automatic playlist evidence. They become preferred
  human context only when the operator explicitly accepts individual or selected
  suggestions through the existing review transaction, which copies the tag to
  `track_user_tags`.
- Keep `track_user_tags` database-only and independent from media metadata such as
  album, artist, year, and genre. Review scoped results in one modal, allow canonical
  playback audition, preselect high/medium-confidence suggestions only, and require an
  explicit accept/reject transaction before changing the database mood tags.

## Options considered

### Let the model write database mood tags directly

This is operationally simple but removes provenance, review, and operator
control. Rejected.

### Allow arbitrary model-created vocabulary

This may surface creative labels but quickly creates synonyms, spelling drift,
and one-off tags that undermine filtering. Rejected. Operators may deliberately
edit the controlled vocabulary, while models may only choose its stable IDs.

### Send database mood tags and stored local generated tags as context

This could improve consistency but expands disclosure and makes accepting one
suggestion change the input signature for every remaining suggestion. Rejected.
The selected path recomputes a privacy-reduced deterministic metadata-and-relative-path
hypothesis from disclosed fields, so it cannot expose the media root or a past review
decision.

### Store a separate model-tag table and review system

This preserves provenance but duplicates the analyzer and review lifecycle that
already supports versioned sources. Rejected.

### Add a versioned analyzer to the existing review pipeline

This keeps one generated-tag ownership model, one explicit promotion path, and
clear source identity while allowing independent local and model suggestions.
Selected.

## Consequences

- Local and model suggestions can appear together without either overwriting
  database mood tags or automatically affecting playlists.
- A large library may require many provider calls; the readiness endpoint shows
  the scoped track count, remaining track count, and estimated batch count before consent.
- Vocabulary edits are visible and revisioned. They invalidate generated model-tag
  profiles and in-flight results without deleting operator-owned database mood tags.
- Closing or refreshing the page does not stop the server job. A server restart
  ends an uncertain job, but already committed batches are reused by the next
  deliberate run.
- Folder and selected-track scopes make it practical to validate a representative
  sample before paying for a whole-library run. Review can audition tracks without
  leaving the tagging workflow.
- Models still receive JSON evidence, not audio. Specialized models that ingest
  audio files require the separate `audio-input/v1` capability, file disclosure,
  quality suite, and cost decision later.
