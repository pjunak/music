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

## Security / correctness — deferred from the 2026-07-09 audit

These surfaced in the full sweep and were left unfixed on purpose — each is
either a design decision the operator should make or a subtle change whose risk
outweighs its low severity. The high/critical findings from that sweep were all
fixed (see git log around that date).

- **Guest library read-enumeration — ACCEPTED, won't fix.** The guest-reachable
  `GET /api/library/tracks/{id}`, `/stream`, `/cover`, and the batch endpoint let
  anyone who can reach the server read/download the library. The operator has
  explicitly accepted read exposure (2026-07-09): it's a personal library, and
  the boundary that matters is *write* access. Write access is fully locked down —
  every filesystem/DB-mutating endpoint requires an authenticated session (audited
  2026-07-09; the only unauthenticated mutation is `POST /login`), and the guest WS
  surface is `register`/`position_report` only, neither of which writes to the
  library. Do not re-open this as a finding.
- **Session tokens stored unhashed at rest (low, defense-in-depth).** The token is
  the primary key of `auth_sessions`. Hashing it (`sha256`, look up by hash) would
  break the Settings → Active Sessions list/revoke UI, which matches on token
  *prefix*. Tokens are 384-bit and the snapshot leak that made them reachable is
  fixed, so this is low-priority.
- **Verbose exception handler (low, intentional).** `main.py`'s catch-all and the
  WS dispatch return `{type}: {message}` to the client — a deliberate single-user
  debugging aid, but it discloses paths/SQL to anonymous guests on an exposed
  instance. Keep for now; revisit if the app is ever multi-user or public.
- **Broadcast happens just outside the state lock (low, subtle).** Two concurrent
  mutators can interleave their per-socket sends so a socket briefly sees an older
  revision; it self-heals on the next broadcast. Fixing it (send under the lock, or
  sequence per socket) is delicate — not worth the risk at single-operator scale.
- **WS upgrade has no Origin check (low, defense-in-depth).** Cross-site WS hijack
  is already blocked by SameSite=lax + the guest gate; an allowlisted-origin check
  on upgrade would be belt-and-suspenders.
- **Pin base images / CI actions by digest (low).** `Dockerfile` and the workflow
  use mutable tags (`python:3.12-slim`, `actions/checkout@v7`).

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
