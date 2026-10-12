import type { KnownDevice } from "@/core/types";
import type * as Http from "@/generated/http";

import { api } from "./http";

export type ActiveSession = Http.ActiveSessionResponse;

export const authApi = {
  listSessions: () => api.get<ActiveSession[]>("/api/auth/sessions"),
  revokeSession: (sessionId: string) =>
    api.delete<void>(
      `/api/auth/sessions/${encodeURIComponent(sessionId)}`,
    ),
};


export const devicesApi = {
  list: () => api.get<KnownDevice[]>("/api/devices"),
  /** Remember a device (or update its name / output designation). */
  save: (clientId: string, payload: { name: string; is_output: boolean }) =>
    api.put<KnownDevice>(`/api/devices/${encodeURIComponent(clientId)}`, payload),
  remove: (clientId: string) =>
    api.delete<void>(`/api/devices/${encodeURIComponent(clientId)}`),
};
