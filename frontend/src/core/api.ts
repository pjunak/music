import type {
  FolderEntry,
  ModeDetail,
  ModeSummary,
  PlaylistMeta,
  Track,
  TrackInPlaylist,
  TreeResponse,
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
      // fall through — surfaced as the error detail below.
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

// --- typed helpers per resource -----------------------------------------

export const modesApi = {
  list: () => api.get<ModeSummary[]>("/api/modes"),
  get: (id: string) => api.get<ModeDetail>(`/api/modes/${encodeURIComponent(id)}`),
};

export interface PresetEffect {
  type: string;
  [key: string]: unknown;
}

export interface PresetManifest {
  id: string;
  name: string;
  description?: string | null;
  effects: PresetEffect[];
}

export const presetsApi = {
  list: () => api.get<PresetManifest[]>("/api/presets"),
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
  create: (payload: { name: string; mode_id?: string | null; category?: string | null }) =>
    api.post<PlaylistMeta>("/api/playlists", payload),
  delete: (playlistId: number) => api.delete<void>(`/api/playlists/${playlistId}`),
  addTrack: (playlistId: number, trackId: number, position?: number) =>
    api.post<TrackInPlaylist>(`/api/playlists/${playlistId}/tracks`, {
      track_id: trackId,
      position,
    }),
  removeTrack: (playlistId: number, position: number) =>
    api.delete<void>(`/api/playlists/${playlistId}/tracks/${position}`),
};

export type LibrarySortKey =
  | "title"
  | "artist"
  | "album"
  | "album_artist"
  | "year"
  | "length_s"
  | "track_no"
  | "added_at"
  | "path";
export type SortOrder = "asc" | "desc";

export interface SearchParams {
  q?: string;
  limit?: number;
  offset?: number;
  sort?: LibrarySortKey;
  order?: SortOrder;
}

export interface SearchResponse {
  tracks: Track[];
  total: number;
  limit: number;
  offset: number;
  sort: LibrarySortKey;
  order: SortOrder;
}

export interface UploadResult {
  saved: Track[];
  destination: string;
}

export interface RescanResult {
  added: number;
  updated: number;
  removed: number;
  unchanged: number;
}

export interface MetadataUpdate {
  title?: string;
  artist?: string;
  album_artist?: string;
  album?: string;
  track_no?: number | null;
  year?: number | null;
  genre?: string;
}

export const libraryApi = {
  getTrack: (id: number) => api.get<Track>(`/api/library/tracks/${id}`),
  search: (params: SearchParams = {}) => {
    const qs = new URLSearchParams();
    if (params.q !== undefined) qs.set("q", params.q);
    if (params.limit !== undefined) qs.set("limit", String(params.limit));
    if (params.offset !== undefined) qs.set("offset", String(params.offset));
    if (params.sort !== undefined) qs.set("sort", params.sort);
    if (params.order !== undefined) qs.set("order", params.order);
    const query = qs.toString();
    return api.get<SearchResponse>(
      query ? `/api/library/search?${query}` : "/api/library/search",
    );
  },
  tree: (path = "") =>
    api.get<TreeResponse>(
      path ? `/api/library/tree?path=${encodeURIComponent(path)}` : "/api/library/tree",
    ),
  upload: (
    files: File[],
    dest: string,
    onProgress?: (loaded: number, total: number) => void,
  ): Promise<UploadResult> => {
    const form = new FormData();
    for (const f of files) form.append("files", f, f.name);
    return new Promise<UploadResult>((resolve, reject) => {
      const xhr = new XMLHttpRequest();
      const path = `/api/library/upload?dest=${encodeURIComponent(dest)}`;
      xhr.open("POST", `${BASE}${path}`);
      xhr.withCredentials = true;
      xhr.responseType = "text";
      xhr.upload.onprogress = (e) => {
        if (onProgress && e.lengthComputable) onProgress(e.loaded, e.total);
      };
      xhr.onerror = () => reject(new Error("network error during upload"));
      xhr.onload = () => {
        if (xhr.status >= 200 && xhr.status < 300) {
          try {
            resolve(JSON.parse(xhr.responseText) as UploadResult);
          } catch {
            reject(new Error("upload succeeded but response wasn't JSON"));
          }
          return;
        }
        let detail = `${xhr.status}: ${xhr.statusText}`;
        try {
          const body = JSON.parse(xhr.responseText) as { detail?: unknown };
          if (body && typeof body.detail === "string") detail = body.detail;
        } catch {
          if (xhr.responseText) detail = xhr.responseText.slice(0, 200);
        }
        reject(new ApiError(xhr.status, detail));
      };
      xhr.send(form);
    });
  },
  rescan: () => api.post<RescanResult>("/api/library/rescan"),
  updateMetadata: (id: number, payload: MetadataUpdate) =>
    api.patch<Track>(`/api/library/tracks/${id}/metadata`, payload),
  moveTrack: (id: number, destination: string, newFilename?: string) =>
    api.post<Track>(`/api/library/tracks/${id}/move`, {
      destination,
      new_filename: newFilename,
    }),
  deleteTrack: (id: number) => api.delete<void>(`/api/library/tracks/${id}`),
  coverUrl: (id: number) => `${BASE}/api/library/tracks/${id}/cover`,
  streamUrl: (id: number) => `${BASE}/api/library/tracks/${id}/stream`,
};

// Re-export types we already had so callers don't need to dig in /core/types.
export type { FolderEntry };
