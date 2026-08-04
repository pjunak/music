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

## Future feature

- **Weighted shuffle.** Reintroduce `"weighted"` only with a real play-count or
  recency algorithm. Persisted legacy values already coerce to `"random"`, so
  the protocol addition can remain additive.
