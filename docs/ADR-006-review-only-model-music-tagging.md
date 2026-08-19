# ADR-006: Review-only model metadata tagging

**Status:** Accepted

**Date:** 2026-08-19
**Decider:** Project owner

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
  analyzer ID `model-metadata-tagger/v1`. Do not create a parallel AI-tag store.
- Limit model input to numeric track ID, indexed title, display title, artist,
  album, origin, genre, duration, and BPM. Do not send paths, audio, signal
  measurements, manual tags, local generated tags, playlists, or review history.
- Send at most 20 tracks per provider request. Treat every metadata string as
  untrusted prompt data and require one output profile for every input ID.
- Restrict output to zero through eight tags from the existing D&D starter
  vocabulary, bounded energy/brightness/tension values, confidence, and short
  evidence. Reject unknown tags, malformed output, missing IDs, duplicate IDs,
  extra IDs, and truncated responses.
- Require the exact current `music-tagging-quality-v1` certification and a
  versioned disclosure confirmation before enqueueing live work. Recheck the
  role fingerprint and quality gate around every provider batch and database
  commit.
- Bind each profile source signature to the consumed track metadata and exact
  model-role runtime fingerprint. Changed metadata or model settings make old
  suggestions stale rather than silently current.
- Run the library pass as a durable, non-restartable job. Commit completed
  batches, skip unchanged profiles, and do not automatically repeat a provider
  call after a server restart. A deliberate later run safely skips committed
  current profiles.
- Keep model profiles out of automatic playlist evidence. They become preferred
  human context only when the operator explicitly accepts individual or selected
  suggestions through the existing review transaction, which copies the tag to
  `track_user_tags`.

## Options considered

### Let the model write manual tags directly

This is operationally simple but removes provenance, review, and operator
control. Rejected.

### Allow arbitrary model-created vocabulary

This may surface creative labels but quickly creates synonyms, spelling drift,
and one-off tags that undermine filtering. Rejected for the first contract;
custom manual tags remain available.

### Send manual and local generated tags as context

This could improve consistency but expands disclosure and makes accepting one
suggestion change the input signature for every remaining suggestion. Rejected.

### Store a separate model-tag table and review system

This preserves provenance but duplicates the analyzer and review lifecycle that
already supports versioned sources. Rejected.

### Add a versioned analyzer to the existing review pipeline

This keeps one generated-tag ownership model, one explicit promotion path, and
clear source identity while allowing independent local and model suggestions.
Selected.

## Consequences

- Local and model suggestions can appear together without either overwriting
  manual tags or automatically affecting playlists.
- A large library may require many provider calls; the readiness endpoint shows
  the remaining track count and estimated batch count before consent.
- Closing or refreshing the page does not stop the server job. A server restart
  ends an uncertain job, but already committed batches are reused by the next
  deliberate run.
- Specialized audio models require a separate capability, file disclosure,
  quality suite, and cost decision later.
