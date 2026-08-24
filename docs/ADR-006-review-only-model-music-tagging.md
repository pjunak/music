# ADR-006: Review-only model evidence tagging

**Status:** Accepted; provider-input details superseded by ADR-008

**Date:** 2026-08-19
**Decider:** Project owner

**Implementation update (2026-08-23):** Tagging is now scope-aware, may use the
canonical library-relative path as bounded untrusted evidence, and is launched from a
playback-capable Library review dialog. The accepted tag store remains database-only.

**Implementation update (2026-08-24):** ADR-008 replaces the former local metadata-hypothesis and
`local-audio/v1` axis input with comprehensive factual `local-context/v1`
evidence. Output is now ID-only `model-context-tagger/v6`; the review-only ownership and
promotion decisions in this ADR remain unchanged.

**Implementation update (2026-08-25):** Input contract v14 places tracks before one
grouped vocabulary whose IDs, meanings, aliases, and global semantic context cues are
co-located instead of duplicated across two lookup tables. The prompt uses an explicit
classify/map/audit procedure and warns that its empty output example demonstrates shape,
not desired sparsity. It still sends no per-track candidate-tag hypothesis. A disclosed,
run-scoped budget allows at most two fresh correction requests for contract-invalid output;
the server never edits or coerces a rejected answer.

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
  analyzer ID `model-context-tagger/v6`. Do not create a parallel AI-tag store.
- Limit model input to numeric track ID, indexed title, display title, artist,
  album, origin, genre, canonical library-relative path, duration, BPM, and an optional bounded
  projection of current `local-context/v1`: whole-track trajectories, tempo development,
  major sections and transitions, repetition, confidence, and optional local
  voice/instrumental classification. Send no locally inferred candidate tag IDs. A non-empty
  display title is canonical for title interpretation. Treat every
  relative path and metadata string as untrusted data. Do not send the absolute media
  root, paths outside the indexed library, audio, waveforms, full-resolution timelines,
  spectrograms, database mood tags, stored generated tags, playlists, or review history.
  Bounded factual context may refine generic mood and activity judgments but is
  never proof of an instrument, genre, setting, scene, or D&D context.
- Send at most 20 tracks per provider request. Treat every metadata string as
  untrusted prompt data and require one output profile for every input ID.
- Resolve scope before provider work. A run may target the whole library, a folder
  with explicit recursive or direct-child behavior, or an explicit set of track IDs.
- Store one revisioned operator-managed vocabulary with stable IDs, normalized names,
  selection definitions, groups, exact cleanup aliases, and overlapping local context
  cues. Every built-in tag has a small set of high-signal soundtrack context cues.
  Send one full bounded group structure with every ID beside its name, definition, exact
  aliases, and bounded semantic context cues in each metadata batch. Cues are global vocabulary
  guidance rather than per-track candidate IDs: the model must confirm them against the
  complete metadata phrase, and they never act as cleanup mappings. Restrict output to zero
  through eight IDs from the current vocabulary, confidence, and short evidence. Inject the exact
  track and tag ID enums into the
  provider schema, then resolve validated IDs to names locally. Deterministically retain
  only the first four bounded, well-typed evidence strings because they are incidental
  review text. Reject unknown tags, invalid confidence, malformed core output,
  missing IDs, duplicate IDs, extra fields, and truncated responses.
- Require the exact current `music-tagging-quality-v1` certification and a
  versioned disclosure confirmation before enqueueing live work. Recheck the
  role fingerprint and quality gate around every provider batch and database
  commit. The synthetic suite repeats each safety scenario once and separates
  blocking output-safety failures from scored semantic recall: provider/contract
  failures and any forbidden false positive on either attempt remain blocking,
  while required-tag misses—including those in safety scenarios—contribute to a
  90% scored pass floor. The operator may recheck only failed cases from the exact
  current complete report; the server merges them with that report before
  recomputing certification.
- Bind each profile source signature to the consumed track metadata, optional
  local-audio source signature, vocabulary fingerprint, input-contract version, and
  exact model-role runtime fingerprint. Changed metadata, audio evidence, vocabulary,
  or model settings make old suggestions stale rather than silently current. Adding a
  first current audio profile also invalidates a metadata-only model result.
- Run the library pass as a durable, non-restartable job. Commit completed
  batches, skip unchanged profiles, and do not automatically repeat a provider
  call after a server restart. A deliberate later run safely skips committed
  current profiles.
- Within a running job only, allow at most two disclosed correction requests after a
  response violates the JSON/tag-ID contract. Do not spend that budget on provider,
  timeout, truncation, or network failures, and never reinterpret invalid core fields.
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
  the scoped track count, remaining track count, estimated batch count, and the
  maximum two additional contract-recovery requests before consent.
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
