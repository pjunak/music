import { useEffect, useState } from "react";
import type { FormEvent } from "react";

import { Field } from "@/components/Field";
import { Modal } from "@/components/Modal";
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

  // Reset fields whenever the modal (re)opens so a previous attempt's text or
  // error can't leak into a fresh open. (Modal handles initial focus.)
  useEffect(() => {
    if (!open) return;
    setUsername("");
    setPassword("");
    setError(null);
    setPending(false);
  }, [open]);

  // Auth resolving to authenticated (here, or via another tab) closes us.
  useEffect(() => {
    if (open && status === "authenticated") setOpen(false);
  }, [open, status, setOpen]);

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
    <Modal
      ariaLabel="Sign in"
      title="Sign in"
      className="login-modal"
      closeButton
      onClose={() => setOpen(false)}
      onSubmit={onSubmit}
      footer={
        <>
          <button type="button" onClick={() => setOpen(false)}>
            Cancel
          </button>
          <button type="submit" className="btn-primary" disabled={pending}>
            {pending ? "Signing in…" : "Sign in"}
          </button>
        </>
      }
    >
      <Field label="Username">
        <input
          type="text"
          data-autofocus
          autoComplete="username"
          value={username}
          onChange={(e) => setUsername(e.target.value)}
          required
        />
      </Field>
      <Field label="Password">
        <input
          type="password"
          autoComplete="current-password"
          value={password}
          onChange={(e) => setPassword(e.target.value)}
          required
        />
      </Field>
      {error !== null ? <p className="error small">{error}</p> : null}
    </Modal>
  );
}
