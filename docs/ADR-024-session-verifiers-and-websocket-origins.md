# ADR-024: Session verifiers and browser WebSocket origins

Status: accepted, 2026-09-05.

## Context

The single-operator server stored reusable session cookies in SQLite and derived
the Settings session identity from their prefixes. A database copy therefore held
live bearer credentials. WebSocket upgrades also relied on cookie/session checks
without inspecting the page's Origin. SameSite cookies do not by themselves reject
an unexpected page on a same-site sibling origin. These are hardening changes;
the audit did not demonstrate a production compromise.

## Decision

The storage authentication boundary hashes every incoming token with SHA-256 and
the `music-session/v1` domain separator. Existing tokens contain 384 random bits,
so a fast one-way verifier is appropriate; this does not change password hashing.
New rows contain only that verifier and an independently generated UUID session ID.
The cookie stays opaque and its name, security flags, lifetime, expiry handling,
last-seen throttling, logout-all behavior and WebSocket rechecks remain unchanged.

Session lists contain no bearer tokens or verifiers. `session_id` identifies a
session for management and `is_current` comes from comparing the current cookie's
verifier inside storage. Revoke uses the complete ID and the authenticated user ID
in one DELETE. Partial IDs and token prefixes are no longer accepted. The existing
`token_prefix` JSON field is a deprecated alias for the complete non-secret ID,
and `/api/auth/sessions/{token_prefix}` retains its legacy parameter name. This
lets existing clients pass the returned value without preserving secret-derived
identity. New UI code uses `session_id`; neither field authenticates.

Schema 11 deliberately replaces the exact recognized legacy session table and
invalidates its logins. Bootstrap preserves its normal verified pre-migration
backup before making this change. Simply hashing still-valid legacy sessions
would leave reusable cookies in that backup. Accounts and authored/library data
are retained. Users sign in again once. Subsequent startup does not reset new
sessions. Malformed table shapes and a current migration ledger with an old table
remain errors, not implicit permission to reset data.

Old backups remain sensitive and unchanged. This migration invalidates their
legacy session cookies on the upgraded server; it does not claim physical erasure
of old database/WAL blocks or protect a deliberate rollback to old software.

Every browser Origin must be one strict HTTP(S) origin. Reject missing syntax,
`null`, empty values, multiple header values or lists, credentials, paths, queries,
fragments and invalid host/port forms. Normalize host case and default ports. Allow:

1. The request Host with the public scheme declared by `SESSION_COOKIE_SECURE`
   (HTTPS when true, HTTP when false), following the existing deployment contract.
2. An explicit, normalized `ALLOWED_ORIGINS` entry.

Apply the check before authentication and every upgrade branch. Return HTTP 403
with `websocket_origin_denied` on rejection. Native clients may omit Origin, so
the existing session and action-authorization checks remain authoritative there.
Origin checks do not authenticate a native client.

Do not trust `Forwarded` or `X-Forwarded-*` without an explicit proxy trust model.
Normal Host-preserving HTTPS proxies work with secure cookies. Host-rewriting
proxies and separately hosted browser controllers need the public page origin in
`ALLOWED_ORIGINS`. Local `file:` controller pages have opaque origins and must be
served from a permitted HTTP(S) origin instead.

## Validation and operational boundary

Regressions cover token/hash/ID substitution, snapshot contents, stable IDs after
reopen, targeted revocation, expiry/logout, legacy reset with an unexpired token,
preserved backup/accounts, malformed schema rejection, and browser/native upgrade
controls. Existing authenticated-socket tests retain session-loss and mutation checks.
Settings tests verify that display shortening never becomes partial-ID revocation.

Baton consumes login/me/logout and an opaque cookie; it does not consume the
session-management DTO. Its wire models remain unchanged. Deployment acceptance
still checks the real proxy, a new browser/Baton login, and physical output behavior.
