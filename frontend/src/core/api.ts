import type {
  ModeSummary,
  PlaylistMeta,
  Track,
  TrackInPlaylist,
} from "@/core/types";

const BASE = import.meta.env.VITE_API_BASE_URL ?? "";

export class ApiError extends Error {
  constructor(
    public status: number,
    public detail: string,
  ) {
    super(`API ${status}: ${detail}`);
  }
}

async function request<T>(method: string, path: string, body?: unknown): Promise<T> {
  const init: RequestInit = { method, credentials: "include" };
  if (body instanceof FormData) {
    // Let the browser set Content-Type with the multipart boundary.
    init.body = body;
  } else if (body !== undefined) {
    init.headers = { "Content-Type": "application/json" };
    init.body = JSON.stringify(body);
  }
  const response = await fetch(`${BASE}${path}`, init);

  if (response.status === 204) {
    return undefined as T;
  }

  const text = await response.text();
  const contentType = response.headers.get("content-type") ?? "";
  const looksLikeJson = contentType.includes("application/json");

  let parsed: unknown = undefined;
  if (text && looksLikeJson) {
    try {
      parsed = JSON.parse(text);
    } catch {
      // fall through — we'll surface the body as the error detail below.
    }
  }

  if (!response.ok || (text && !looksLikeJson)) {
    const fromJson =
      parsed && typeof parsed === "object" && parsed !== null && "detail" in parsed
        ? String((parsed as { detail: unknown }).detail)
        : null;
    const snippet = text.slice(0, 200).replace(/\s+/g, " ").trim();
    const detail =
      fromJson ??
      (looksLikeJson
        ? response.statusText
        : `non-JSON response (${contentType || "no content-type"}): ${snippet}`);
    throw new ApiError(response.status, detail);
  }

  return parsed as T;
}

export const api = {
  get: <T>(path: string) => request<T>("GET", path),
  post: <T>(path: string, body?: unknown) => request<T>("POST", path, body),
  put: <T>(path: string, body?: unknown) => request<T>("PUT", path, body),
  patch: <T>(path: string, body?: unknown) => request<T>("PATCH", path, body),
  delete: <T>(path: string) => request<T>("DELETE", path),
};

// --- Typed helpers per resource ------------------------------------------

export const modesApi = {
  list: () => api.get<ModeSummary[]>("/api/modes"),
};

export const playlistsApi = {
  list: (params: { mode_id?: string; category?: string } = {}) => {
    const q = new URLSearchParams();
    if (params.mode_id !== undefined) q.set("mode_id", params.mode_id);
    if (params.category !== undefined) q.set("category", params.category);
    const qs = q.toString();
    return api.get<PlaylistMeta[]>(`/api/playlists${qs ? `?${qs}` : ""}`);
  },
  tracks: (playlistId: number) =>
    api.get<TrackInPlaylist[]>(`/api/playlists/${playlistId}/tracks`),
};

export interface IncomingFile {
  name: string;
  size_bytes: number;
  modified_at: string;
}

export interface IngestResult {
  ok: boolean;
  returncode: number;
  stdout: string;
  stderr: string;
}

export const libraryApi = {
  search: (q: string, limit = 50) => {
    const qs = new URLSearchParams({ q, limit: String(limit) });
    return api.get<{ tracks: Track[]; limit: number; offset: number }>(
      `/api/library/search?${qs.toString()}`,
    );
  },
  listIncoming: () => api.get<{ files: IncomingFile[] }>("/api/library/incoming"),
  upload: (files: File[]) => {
    const form = new FormData();
    for (const f of files) form.append("files", f, f.name);
    return api.post<{ saved: IncomingFile[] }>("/api/library/upload", form);
  },
  deleteIncoming: (filename: string) =>
    api.delete<void>(`/api/library/incoming/${encodeURIComponent(filename)}`),
  ingest: () => api.post<IngestResult>("/api/library/ingest"),
};
