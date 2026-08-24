# Backlog

Only actionable, deliberately deferred work belongs here. Completed items are
deleted; accepted product/security decisions live in `README.md` or `AGENTS.md`.

## Next

- **Remove the SPA end-of-track stall backstop.** The server-side advancer has
  existed since 2026-07-08. Once its production behavior is confirmed, delete
  `maybeAdvanceAtEnd`, `endStallTime`, and `ADVANCE_DEBOUNCE_MS` from
  `frontend/src/core/playbackEngine.ts`. Preserve the low-latency `ended` → skip
  path.

## Hardening

- **Hash session tokens at rest.** This requires replacing the Settings UI's
  token-prefix identity with a separate stable session identifier.
- **Sequence broadcasts per socket.** Concurrent commits can currently deliver
  an older revision immediately after a newer one; clients self-heal on the next
  snapshot, but strict ordering would remove the transient regression.
- **Validate WebSocket origins.** Add an allowlist check as defense in depth on
  top of SameSite cookies and the guest mutation gate.
- **Pin container bases and CI actions by digest** if supply-chain
  reproducibility becomes more important than automatic patch updates.

## Maintenance

- **Release the database session during MusicBrainz lookups.** `verify_names`
  can hold a session for roughly 11 seconds while paced remote calls run;
  separate scoring from the final write transaction.
- **Make the headless reconciler independently testable.** Extract playback
  state reconciliation from mpv/WebSocket plumbing and add unit tests.

## Code health priorities

The 2026-08-25 sweep scored remaining structural debt as
`(impact + risk) × (6 - effort)`, each input on a 1–5 scale. These are deliberate
follow-up refactors, not permission to weaken current validation or combine safety
boundaries merely to reduce line counts.

| Score | Area | Evidence and risk | Next safe slice |
|---:|---|---|---|
| 30 | Model-tagging job orchestration | `run_model_music_tagging` mixes gate rechecks, batching, retry accounting, persistence, and progress in one long function. A future contract change can miss one boundary. | Extract typed preflight, one-batch execution, and atomic persistence helpers while keeping the existing end-to-end tests unchanged. |
| 18 | Generated-tag bulk review | `review_analysis_tags_bulk` combines stale checks, per-track limits, review transitions, and manual-tag writes. This is operator-owned data with many partial-result branches. | Separate pure validation/planning from the one locked transaction; add table-driven tests before moving code. |
| 18 | Authoring import commit | `commit_bundle` owns dependency validation and several resource writers in one transaction. It is correct but expensive to extend safely. | Extract resource-specific create helpers behind the existing preview/selection/dependency contract. |
| 16 | Library cleanup apply/revert | `_apply_op` and `_revert_item` branch across file and metadata mutations. Drift handling is safety-critical and difficult to audit as one dispatcher. | Introduce typed operation handlers one operation kind at a time, preserving the journal format and rollback tests. |
| 15 | Frontend main bundle | The production build emits a 603 kB minified main chunk, above Vite's 500 kB warning threshold. This increases initial transfer/parse work even though AI setup is already lazy-loaded. | Profile the module graph and lazy-load infrequent Authoring/Diagnostics surfaces; do not merely raise the warning threshold. |

## Future feature

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
