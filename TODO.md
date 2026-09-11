# Backlog

Only actionable, deliberately deferred work belongs here. Completed items are
deleted; accepted product/security decisions live in `README.md` or `AGENTS.md`.

## Scope and ownership

The approved audit fixes and bounded cleanup are implemented. Remaining release
and AI-role checks, with the engineering/operator split, are in
[the acceptance plan](docs/AI_ACCEPTANCE.md). They do not require the operator to
write code or design tests. Further refactoring should accompany a concrete change
or demonstrated defect rather than extend the cleanup phase indefinitely.

## Library metadata: ordered implementation plan

The core cleanup, compilation safeguards, informative collision names, album-scoped
retrieval, corroborated discovery within and across explicit disc folders, catalog artist aliases,
an accessible rejection pool, catalog exports, incomplete-result summaries, safe provider diagnostics
and offline pilot scoring/comparison tooling are implemented. This is the remaining plan for
albums and game/film soundtracks, ordered by expected useful corrections, protection of correct metadata,
review effort and implementation cost. Benefits are engineering estimates;
they are not measured accuracy gains. The [research](docs/LIBRARY_METADATA_RESEARCH.md)
records the alternatives and sources. Completed work moves into the living
Assistant contract and is removed from this backlog.

| Order | Work and expected usefulness | Downsides and mitigation | Effort / next step |
|---|---|---|---|
| 1 | Finish the fixed album/soundtrack pilot with the [implemented tooling](docs/LIBRARY_METADATA_PILOT.md). Establish recording/edition precision, missed candidates, harmful field changes and abstention before changing retrieval. Evaluate available metadata proposals even when optional tag evidence is incomplete. | Independent labels require operator time; small or album-leaking samples mislead. Keep artist/release families together, report denominators and unknowns; partial evidence never counts as a complete no-match. | Small–medium; complete missing baseline runs and independent recording/edition judgments, preserving the fixed holdout. |
| 2 | Assess MusicBrainz timeouts affecting recording and edition coverage. Recovering missing evidence comes before relaxing matching. | A timeout alone does not identify pacing, provider load or a network problem. Blind retries add latency and traffic; an unavailable response is not a no-match judgment. | Small–medium; use the retained baseline's failed request categories to choose a bounded availability diagnostic before changing retry policy. |
| 3 | Evaluate alias-assisted identity acceptance and explicit version evidence. Helps tracks still unresolved after catalog alias retrieval, album hints and bounded disc-folder discovery. | Accepting alternate artist spellings can collapse distinct identities. Preserve full credits, retain provenance and reject version/identity contradictions. Conservative encoding repair is an optional substep, never blanket normalization. | Large; use independently labeled misses before relaxing acceptance or adding speculative normalization. |
| 4 | Writable richer metadata: full/original dates, artist lists, credits, work/movement, label and track totals. Useful for soundtracks/classical libraries and export. | More complicated forms, tag-format differences and cross-client schemas; some catalog facts belong to a work rather than a recording. Choose a small useful field set before changing playback/client models; preserve original values and unknown frames. | Large; consult on fields and whether they need tag export, browsing, or both. |
| 5 | CUE sheets and purchase/creator manifests. Recovers authoritative local track lists and source context. | Format-specific parsing, private receipt data and uncertain file mapping. Explicit selected imports only; never execute sidecars or split audio implicitly. | Medium; prioritize actual formats present in the collection. |
| 6 | Exact-file duplicate groups and a comparison/review screen. Finds wasted copies without equating same titles with same audio. | Reading whole files costs I/O. Same audio can carry different tags/artwork; deleting a copy can damage playlists or references. Start with read-only size/hash grouping; discuss retention and deletion before adding actions. Decoded/acoustic comparison is a later, separate experiment. | Medium–large; consult on duplicate retention workflow. |
| 7 | Fingerprint reuse after renames/tag-only edits. Saves repeated analysis on large libraries. | Audio-content identity and persistent cache invalidation are more complex than path/stat keys; decoding or hashing can itself be expensive. Profile first and retain parameter/version keys. | Medium; conditional on measured repeated work. |
| 8 | One additional catalog or paid recognition fallback. May resolve cases missed by current sources. | Coverage is unknown; credentials, requests, attribution, persistence/export terms and possible charges add maintenance. Compare incremental useful matches on the same unresolved cohort. | Medium–large; consult on a concrete provider, permitted data, and request/cost cap. |
| 9 | Broader AI text parsing, booklet OCR and audio/ambience models. Could help niche material with little catalog coverage. | Hallucinations/OCR errors, private content disclosure, runtime/model size, licensing and uncertain benefit. Separate experiments with evidence references, abstention, explicit review and small evaluation budgets. | Large; consult after benchmark results justify a specific experiment. |

Albums and soundtracks put edition/position recovery, full dates and credits ahead
of duplicate cleanup and ambience models. Richer writable fields require a choice
of the first field set and its tag-export/browsing use before shared schemas change.
Paid recognition and broader AI stay deferred pending a concrete gap, proposed
provider/data scope and cost cap. Normal regression gates establish correctness,
not recognition accuracy on the collection; field judgments alone do not establish recording or edition identity.

Last.fm's configured-key control succeeded, while the tested recording-ID query
returned HTTP 400 and its exact name query returned no tags. The operator chose to
keep recording-ID matching strict and prioritize album metadata. A separately
labeled name fallback remains deferred; this sample showed no useful added tags,
and name equality cannot establish a particular recording version. Do not rotate
the working key, disable the source, or block the album pilot on this optional
tag gap. Its exact error payload remains unclassified; keep the retained diagnostic
and [interpretation limits](docs/LIBRARY_METADATA_PILOT.md#when-an-http-error-has-no-api-code).

## Conditional cleanup

- **Remove the SPA end-of-track stall backstop.** The server-side advancer has
  existed since 2026-07-08. Once its production behavior is confirmed, delete
  `maybeAdvanceAtEnd`, `endStallTime`, and `ADVANCE_DEBOUNCE_MS` from
  `frontend/src/core/playbackEngine.ts`. Preserve the low-latency `ended` → skip
  path.

- **Pin container bases and CI actions by digest** if supply-chain
  reproducibility becomes more important than automatic patch updates.
- **Provider queue fairness.** If quick drafts wait unacceptably behind bulk work,
  use the existing timing diagnostics and implement bounded request scheduling.
  No scheduler redesign or formal load study is required just to close this audit.
- **Intermittent WebSocket timeout.** Investigate if it recurs; retain phase labels
  and existing assertions. Do not treat a non-reproducing test failure as an ongoing
  implementation task.
- **Shared proposal provenance.** Unify model/catalog presentation only if the
  existing review workflows demonstrate a concrete benefit.

## Mood quality evaluation

- **Run the fixed listening pilot before scaling.** The diagnostic UI, selected
  reconsideration, 20-track/no-tag guard, compact input and context-only quality
  gate are implemented. The operator supplies independent judgments; engineering
  compares cost/usefulness and profiles the first music-classifier candidate.
  Integrate/cache a winner only after compatibility and listening evidence.
  See [the offline pilot and scorer](docs/MOOD_PILOT.md). No cleanup certification
  or repeat algorithmic analysis is needed just to inspect existing AI results.
- **Calibrate acoustic context.** Compare gain-normalized intensity evidence, a finer tempo
  estimator, and present context on a small reviewed sample before another full-library model
  pass. The spectral coverage defect is fixed; intensity still includes recording volume,
  tempo is approximate and classifier confidence is not calibrated mood accuracy.
  [The context review](docs/MOOD_CONTEXT_REVIEW.md) defines the engineering/operator split.
- **Measure cost per useful accepted tag.** Compare metadata-only and metadata-plus-context
  with identical scope/model/Thinking/limits; record unsupported tags and abstentions, not only
  schema validity. Keep new audio encoders or vocabulary pruning conditional on these results.

## New features — deferred by the operator

- **Broader EQ assistance.** Revisit the desired effect/control scope after the
  AI usefulness evaluation. The current assistant drafts the fixed ten-band EQ;
  expanded controls and EQ acceptance are explicitly deferred.
- **Specialized model audio analysis.** Choose a concrete provider protocol,
  then add a bounded `audio-input/v1` adapter, explicit file disclosure and
  consent, a synthetic quality suite, durable non-restartable execution, and a
  review-only result contract. Do not unlock the reserved role before all of
  those boundaries exist.
- **Provider-independent cost controls.** Provider dashboards remain the source
  of truth for spending limits. The current track/request/reservation limits bound individual tagging runs. Add
  currency or account-wide budgets only with a trustworthy accounting contract;
  never infer charges from missing usage.
- **Weighted shuffle.** Reintroduce `"weighted"` only with a real play-count or
  recency algorithm. Persisted legacy values already coerce to `"random"`, so
  the protocol addition can remain additive.
