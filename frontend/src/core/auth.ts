import { create } from "zustand";

import { ApiError, api, setUnauthorizedHandler } from "@/core/api";
import { toast } from "@/core/toast";
import { useUiTransient } from "@/core/uiTransient";

export type AuthStatus = "unknown" | "authenticated" | "anonymous";

export interface UserInfo {
  id: number;
  username: string;
}

interface AuthState {
  status: AuthStatus;
  user: UserInfo | null;
  refresh: () => Promise<void>;
  login: (username: string, password: string) => Promise<void>;
  logout: () => Promise<void>;
  /** The session died out from under an authenticated operator (expired or
   *  revoked, detected via a 401 or a coded WS error frame). Flips the store
   *  to anonymous — every `Protected` route swaps to its in-place LoginGate —
   *  and opens the login modal so re-entry is one password away, exactly
   *  where they were. No-op unless currently authenticated, so boot-time /me
   *  probes and wrong-password 401s never trigger it. */
  sessionLost: (kind: "expired" | "revoked") => void;
}

export const useAuthStore = create<AuthState>((set, get) => ({
  status: "unknown",
  user: null,

  refresh: async () => {
    try {
      const user = await api.get<UserInfo>("/api/auth/me");
      set({ status: "authenticated", user });
    } catch (err) {
      if (err instanceof ApiError && err.status === 401) {
        // Definitive "not signed in" — clear the session.
        set({ status: "anonymous", user: null });
      } else if (get().status === "unknown") {
        // Non-401 failure on the *initial* check (server unreachable, 502
        // while the container is still booting, non-JSON from the proxy).
        // We must resolve to something concrete: leaving status "unknown"
        // strands the whole app on a blocking spinner (the "black screen
        // for a few seconds that never recovers"). Resolve to anonymous so
        // the public TV view renders and the login gate offers a retry.
        console.warn("[auth] initial refresh failed (non-401) — treating as anonymous", err);
        set({ status: "anonymous", user: null });
      } else {
        // Same failure but we were ALREADY authenticated — a transient blip
        // shouldn't sign the operator out mid-session. Keep the session until
        // a real 401 arrives. (Without this, a brief WiFi drop used to log the
        // user out of the SPA on the next /api/auth/me call.)
        console.warn("[auth] refresh failed (non-401, keeping current state)", err);
      }
    }
  },

  login: async (username, password) => {
    const user = await api.post<UserInfo>("/api/auth/login", { username, password });
    set({ status: "authenticated", user });
  },

  logout: async () => {
    // Anonymous first: if the session is already dead server-side, the 401
    // from this call routes through the unauthorized handler, and it must
    // read as "already signed out" (no-op) — not pop the re-login modal in
    // the middle of an intentional sign-out.
    set({ status: "anonymous", user: null });
    try {
      await api.post("/api/auth/logout");
    } catch (err) {
      // Local sign-out already took effect; the server-side row (if it even
      // still exists) can be revoked from Settings → Active Sessions later.
      console.warn("[auth] logout call failed (signed out locally)", err);
    }
  },

  sessionLost: (kind) => {
    if (get().status !== "authenticated") return;
    set({ status: "anonymous", user: null });
    useUiTransient.getState().setLoginOpen(true);
    toast.warn(
      "Signed out",
      kind === "revoked"
        ? "This session was revoked — sign in to continue."
        : "Your session expired — sign in to continue.",
    );
  },
}));

// Every 401 the API layer sees routes here; sessionLost itself decides
// whether it means anything (only when the operator was authenticated).
setUnauthorizedHandler(() => {
  useAuthStore.getState().sessionLost("expired");
});
