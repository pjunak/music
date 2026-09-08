# Backlog

Only actionable, deliberately deferred work belongs here. Completed items are
deleted; accepted product/security decisions live in `README.md` or `AGENTS.md`.

## Scope and ownership

The approved audit fixes and bounded cleanup are implemented. Remaining release
and AI-role checks, with the engineering/operator split, are in
[the acceptance plan](docs/AI_ACCEPTANCE.md). They do not require the operator to
write code or design tests. Further refactoring should accompany a concrete change
or demonstrated defect rather than extend the cleanup phase indefinitely.

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
- **Model-assisted library cleanup.** Extend the existing propose -> review ->
  journal -> execute cleanup workflow with a minimized model input and fixed
  output schema. The model must not move, rename, or delete files directly.
- **Provider-independent cost controls.** Provider dashboards remain the source
  of truth for spending limits. The current track/request/reservation limits bound individual tagging runs. Add
  currency or account-wide budgets only with a trustworthy accounting contract;
  never infer charges from missing usage.
- **Weighted shuffle.** Reintroduce `"weighted"` only with a real play-count or
  recency algorithm. Persisted legacy values already coerce to `"random"`, so
  the protocol addition can remain additive.
