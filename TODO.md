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

## New features — deferred by the operator

- **Specialized model audio analysis.** Choose a concrete provider protocol,
  then add a bounded `audio-input/v1` adapter, explicit file disclosure and
  consent, a synthetic quality suite, durable non-restartable execution, and a
  review-only result contract. Do not unlock the reserved role before all of
  those boundaries exist.
- **Model-assisted library cleanup.** Extend the existing propose -> review ->
  journal -> execute cleanup workflow with a minimized model input and fixed
  output schema. The model must not move, rename, or delete files directly.
- **Provider-independent cost controls.** Provider dashboards remain the source
  of truth for spending limits. Add in-app budgets only if adapters can expose a
  trustworthy portable accounting contract; never infer charges from missing
  token usage.
- **Weighted shuffle.** Reintroduce `"weighted"` only with a real play-count or
  recency algorithm. Persisted legacy values already coerce to `"random"`, so
  the protocol addition can remain additive.
