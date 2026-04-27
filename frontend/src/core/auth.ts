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
        set({ status: "anonymous", user: null });
      } else {
        set({ status: "anonymous", user: null });
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
