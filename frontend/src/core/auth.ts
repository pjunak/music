import { create } from "zustand";

import { ApiError, api } from "@/core/api";

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
    try {
      await api.post("/api/auth/logout");
    } finally {
      set({ status: "anonymous", user: null });
    }
  },
}));
