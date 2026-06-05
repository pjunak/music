import { useEffect } from "react";
import type { ReactNode } from "react";
import { Link, Navigate } from "react-router-dom";

import { useAuthStore } from "@/core/auth";
import { useUiTransient } from "@/core/uiTransient";

/** Gate a protected route's *content* — never by navigation.
 *
 *  - authenticated → render it.
 *  - unknown       → a spinner (auth is still resolving; don't flash the
 *                    sign-in gate at someone who turns out to be signed in,
 *                    and don't redirect anywhere).
 *  - anonymous     → an in-place sign-in gate (a button that opens the login
 *                    modal). The URL never moves, so the moment auth flips the
 *                    gated content appears right here with no redirect dance. */
export function Protected({ children }: { children: ReactNode }) {
  const status = useAuthStore((s) => s.status);
  if (status === "authenticated") return <>{children}</>;
  if (status === "unknown") return <RouteSpinner />;
  return <LoginGate />;
}

export function RouteSpinner() {
  return (
    <div className="route-status" role="status" aria-live="polite">
      <span className="spinner" aria-hidden="true" />
      <span className="muted">Loading…</span>
    </div>
  );
}

export function LoginGate() {
  const setLoginOpen = useUiTransient((s) => s.setLoginOpen);
  return (
    <div className="route-status login-gate">
      <h2>Sign in required</h2>
      <p className="muted">This is the operator workspace. Sign in to continue.</p>
      <button
        type="button"
        className="btn-primary"
        onClick={() => setLoginOpen(true)}
      >
        Sign in
      </button>
      <p className="muted small">
        Looking for the room display? <Link to="/tv">Open the TV view</Link>.
      </p>
    </div>
  );
}

/** Back-compat for `/login` bookmarks and old external links: open the modal
 *  and bounce to `/`. Sign-in is no longer a page. Skip opening the modal if
 *  already signed in (the bounce to `/` lands them on /console). */
export function LoginRedirect() {
  const setLoginOpen = useUiTransient((s) => s.setLoginOpen);
  const status = useAuthStore((s) => s.status);
  useEffect(() => {
    if (status !== "authenticated") setLoginOpen(true);
  }, [setLoginOpen, status]);
  return <Navigate to="/" replace />;
}
