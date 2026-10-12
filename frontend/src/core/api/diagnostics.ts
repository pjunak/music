import type { FolderEntry } from "@/core/types";
import type * as Http from "@/generated/http";

import { api } from "./http";

export type LoaderStatus = Http.LoaderStatus;

export type DiagnosticsResponse = Http.DiagnosticsResponse;

export const diagnosticsApi = {
  get: () => api.get<DiagnosticsResponse>("/api/diagnostics"),
};

// Re-export types we already had so callers don't need to dig in /core/types.
export type { FolderEntry };
