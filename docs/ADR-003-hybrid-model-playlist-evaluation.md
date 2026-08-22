# ADR-003: Hybrid model playlist planning behind synthetic evaluation

**Status:** Accepted

**Date:** 2026-08-19
**Decider:** Project owner

**Extended by:** [ADR-005](ADR-005-consent-bound-model-playlist-suggestions.md),
which adds the durable, consent-bound live-library path while preserving this
local filtering and ID-only output contract.

## Context

The optional provider harness can execute strict JSON requests, but connecting
it directly to the live library would expose a large and weakly bounded data
surface before model quality, source integrity, and failure behavior are known.
Allowing a model to author complete suggestion objects would also let it invent
paths, titles, tags, evidence, scores, or explanations that appear server-owned.

## Decision

- Implement the versioned model playlist planner (currently
  `model-playlist-planner/v2`) behind the existing
  `PlaylistSuggestionEngine` protocol, without changing the live Assistant API.
- Run the local planner first with an expanded but bounded candidate limit. It
  remains authoritative for exclusions, BPM eligibility, current analysis,
  manual/generated tag separation, numeric evidence, and source metadata.
- Send the deterministic local rank, default selection, playback sequence, effective
  BPM source, and duration plan so the model refines a complete local baseline rather
  than reconstructing one.
- Send at most 100 candidates to the model. The provider payload excludes
  library-relative paths and local explanation text. Candidate titles, artists,
  albums, origins, genres, tags, and numeric evidence remain explicitly marked
  as untrusted data in the fixed system prompt.
- Accept only one strict output object containing the contract version, ranked
  track IDs, and selected track IDs in playback order. Reject unknown,
  duplicate, unranked-selected, over-limit, malformed, or explicitly truncated
  output. Do not repair it silently.
- Reconstruct every public candidate from the local snapshot. The model cannot
  supply or alter paths, metadata, tags, evidence confidence, scores, reasons,
  audio measurements, or eligibility counts.
- Expose configured-model execution only through `evaluate-playlists` and
  require `--send-suite-to-provider` on every invocation. The operator chooses
  the suite file; the CLI never loads the live library.
- Run the configured model through the same versioned relevance, selection,
  ordering, determinism, exclusion, source-integrity, and candidate-limit checks
  as the local engine. Safe engine error codes may appear in evaluation output;
  credentials, prompts, raw responses, and provider error bodies may not.

## Options considered

### Send the entire live library to the provider

Provides maximum context but creates uncontrolled payload size, disclosure,
latency, and cost before a quality gate exists. Rejected.

### Trust a model-produced `PlaylistSuggestionResponse`

Avoids server reconstruction but lets the model counterfeit local metadata and
evidence. Rejected.

### Filter or silently drop invalid model IDs

Would keep some requests usable but hide contract violations from evaluation
and make a poor model appear safer than it is. Rejected.

### Local prefilter, ID-only model planning, trusted reconstruction

Keeps existing invariants authoritative while allowing a model to contribute
semantic ranking and sequence planning. Selected.

## Consequences

- A model can improve or worsen ordering and selection, but it cannot expand the
  eligible set or modify source data.
- The current checked-in suite may issue two provider calls for cases that
  require determinism, so explicit evaluation may incur cost and rate limits.
- The disclosure flag confirms only the selected suite. Custom suites may still
  contain sensitive text; their author remains responsible for reviewing them.
- Passing the synthetic suite is necessary but not sufficient for live use.
  ADR-005 supplies the separate in-app disclosure and consent step, bounded
  asynchronous execution, local fallback, and existing review-to-import
  workflow. The evaluation CLI itself still sends only the chosen synthetic suite.
