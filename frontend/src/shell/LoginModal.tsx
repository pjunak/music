import { useEffect, useRef, useState } from "react";
import type { FormEvent } from "react";

import { ApiError } from "@/core/api";
import { useAuthStore } from "@/core/auth";
import { useUiTransient } from "@/core/uiTransient";

/** Sign-in as an overlay rather than a route.
 *
 *  The old `/login` page meant every "you need to be signed in" path was a
 *  navigation — which produced the visible redirect ping-pong (console →
 *  login → tv) the operator hit. As a modal, signing in never moves the URL:
 *  the operator stays on whatever protected route they aimed at, and the
 *  moment auth flips to authenticated the gated content renders in place and
 *  this closes itself.
 *
 *  Rendered once at AppShell, like ConfirmDialogHost / InputDialogHost. */
export function LoginModal() {
  const open = useUiTransient((s) => s.loginOpen);
  const setOpen = useUiTransient((s) => s.setLoginOpen);
  const login = useAuthStore((s) => s.login);
  const status = useAuthStore((s) => s.status);

  const [username, setUsername] = useState("");
  const [password, setPassword] = useState("");
  const [error, setError] = useState<string | null>(null);
  const [pending, setPending] = useState(false);
  const userRef = useRef<HTMLInputElement | null>(null);

  // Reset fields whenever the modal (re)opens so a previous attempt's text or
  // error can't leak into a fresh open, and focus the first field.
  useEffect(() => {
    if (!open) return;
    setUsername("");
    setPassword("");
    setError(null);
    setPending(false);
    queueMicrotask(() => userRef.current?.focus());
  }, [open]);

  // Auth resolving to authenticated (here, or via another tab) closes us.
  useEffect(() => {
    if (open && status === "authenticated") setOpen(false);
  }, [open, status, setOpen]);

  // Escape closes — signing in is never destructive, so a stray Esc is safe.
  useEffect(() => {
    if (!open) return;
    function onKey(e: KeyboardEvent) {
      if (e.key === "Escape") setOpen(false);
    }
    window.addEventListener("keydown", onKey);
    return () => window.removeEventListener("keydown", onKey);
  }, [open, setOpen]);

  if (!open) return null;

  async function onSubmit(event: FormEvent) {
    event.preventDefault();
    setError(null);
    setPending(true);
    try {
      await login(username, password);
      // The status effect above closes the modal once the store flips.
    } catch (err) {
      if (err instanceof ApiError && err.status === 401) {
        setError("Invalid credentials.");
      } else {
        setError("Login failed. Check the server is running.");
      }
      setPending(false);
    }
  }

  return (
    <div className="modal-backdrop" onMouseDown={() => setOpen(false)}>
      <form
        className="modal login-modal"
        role="dialog"
        aria-modal="true"
        aria-label="Sign in"
        onMouseDown={(e) => e.stopPropagation()}
        onSubmit={onSubmit}
      >
        <header className="modal-header">
          <h2>Sign in</h2>
          <button
            type="button"
            onClick={() => setOpen(false)}
            aria-label="Close sign-in"
            title="Close"
          >
            ×
          </button>
        </header>
        <div className="modal-body">
          <label className="login-field">
            <span className="muted small">Username</span>
            <input
              ref={userRef}
              autoComplete="username"
              value={username}
              onChange={(e) => setUsername(e.target.value)}
              required
            />
          </label>
          <label className="login-field">
            <span className="muted small">Password</span>
            <input
              type="password"
              autoComplete="current-password"
              value={password}
              onChange={(e) => setPassword(e.target.value)}
              required
            />
          </label>
          {error !== null ? <p className="error small">{error}</p> : null}
        </div>
        <div className="modal-actions">
          <button type="button" onClick={() => setOpen(false)}>
            Cancel
          </button>
          <button type="submit" className="btn-primary" disabled={pending}>
            {pending ? "Signing in…" : "Sign in"}
          </button>
        </div>
      </form>
    </div>
  );
}
