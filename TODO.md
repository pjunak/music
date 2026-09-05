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
- **Validate WebSocket origins.** Add an allowlist check as defense in depth on
  top of SameSite cookies and the guest mutation gate.
- **Pin container bases and CI actions by digest** if supply-chain
  reproducibility becomes more important than automatic patch updates.

## Code health priorities

Updated after the 2026-09-05 audit implementation. Preserve strict validation and
transaction ownership during these follow-ups. Operator acceptance and held-out
model evaluation are tracked in [the validation plan](docs/AI_ACCEPTANCE.md).

| Area | Remaining risk | Next safe slice |
|---|---|---|
| Task result schemas | Static schema structure is authored separately from strict Serde result types. | Derive static structure and retain dynamic identifier/bounds validation; test adversarial schema/validator agreement. |
| Catalog orchestration | Connector policy and mapping still live in the server enrichment module. | Move use cases behind typed catalog ports, following the model-job transport boundary. |
| Generated-tag bulk review | Storage `review_analysis` combines stale checks, per-track limits, review transitions, and manual-tag writes. | Separate pure validation/planning from the single write transaction; preserve partial-result and stale-review tests. |
| Authoring import commit | The authoring commit service validates dependencies and coordinates resource writers. | Extract resource-specific helpers behind the existing preview/selection/dependency contract. |
| Library cleanup apply/revert | File and metadata mutation branches must preserve recovery and drift handling. | Extract one typed operation handler at a time, retaining the journal format and rollback tests. |
| Provider attempt outcomes and queue fairness | Timeouts do not prove a request was unsent; long jobs can delay interactive drafts. | Record explicit attempt states and measure queue delay before changing scheduling or retry policy. |

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
