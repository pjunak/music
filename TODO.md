# TODO

Deferred work, roughly in priority order. Items graduate into commits and get
deleted here — no strikethrough graveyard.

## Near-term

- **Dependabot: ignore TypeScript 7.x** — comment `@dependabot ignore this
  major version` on the open typescript-7 PR. typescript-eslint (every release
  channel, canary included) peer-caps `typescript <6.1.0` and its parser
  crashes under TS 7. Revisit when typescript-eslint declares support; until
  then TS stays pinned at 6.0.3.
- **Delete the SPA stall backstop** — `maybeAdvanceAtEnd` / `endStallTime` /
  `ADVANCE_DEBOUNCE_MS` in `frontend/src/core/playbackEngine.ts`, once the
  server-side advancer (`app/sync/advancer.py`) has soaked in production for a
  couple of weeks. Keep the `ended` → skip fast path — it's the low-latency
  half of the contract.

## Someday

- **Weighted shuffle** — re-add a `"weighted"` shuffle mode backed by an
  actual weighting algorithm (play count / recency). The enum value was
  removed 2026-07-09 because it only ever drew uniformly (same as "random");
  persisted states coerce `"weighted"` → `"random"` on load
  (`_prune_dangling_state`), so re-adding is purely additive.
- **Session-expiry UX** — when the operator session expires mid-use, the SPA
  surfaces raw 401s; it needs a proper re-login flow.
- **Headless client testability** — `clients/headless/music_output.py` has no
  unit tests; extract the state-reconcile logic from the GStreamer/socket
  plumbing so it can run under pytest.
