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

export const useAuthStore = create<AuthState>((set) => ({
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
      } else {
        // Network error / 5xx / non-JSON — server told us nothing
        // useful about auth state. Don't tear down the local session:
        // if we were authenticated, stay so until a real 401 arrives.
        // (Without this, a brief WiFi blip used to log the user out
        // of the SPA on the next /api/auth/me call.)
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
