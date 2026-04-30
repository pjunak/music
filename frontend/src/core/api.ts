import type {
  FolderEntry,
  InterruptSpec,
  ModeDetail,
  ModeSummary,
  PlaylistMeta,
  SceneLoopingSfx,
  SceneSpec,
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
  moveTrack: (playlistId: number, position: number, toPosition: number) =>
    api.patch<void>(`/api/playlists/${playlistId}/tracks/${position}`, {
      to_position: toPosition,
    }),
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
  // DB-only fields — not written to the file's tags. See backend.
  display_title?: string;
  origin?: string;
}

export interface BulkMetadataUpdate {
  track_ids: number[];
  updates: MetadataUpdate;
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
  updateBulkMetadata: (payload: BulkMetadataUpdate) =>
    api.patch<Track[]>("/api/library/tracks/bulk-metadata", payload),
  moveTrack: (id: number, destination: string, newFilename?: string) =>
    api.post<Track>(`/api/library/tracks/${id}/move`, {
      destination,
      new_filename: newFilename,
    }),
  deleteTrack: (id: number) => api.delete<void>(`/api/library/tracks/${id}`),
  coverUrl: (id: number) => `${BASE}/api/library/tracks/${id}/cover`,
  streamUrl: (id: number) => `${BASE}/api/library/tracks/${id}/stream`,
  createFolder: (path: string) =>
    api.post<{ name: string; path: string; track_count: number }>(
      "/api/library/folders",
      { path },
    ),
  deleteFolder: (path: string, recursive: boolean) => {
    const qs = new URLSearchParams({ path, recursive: String(recursive) });
    return api.delete<{ removed_tracks: number }>(
      `/api/library/folders?${qs.toString()}`,
    );
  },
  renameFolder: (src: string, dst: string) =>
    api.post<{ name: string; path: string; track_count: number }>(
      "/api/library/folders/rename",
      { src, dst },
    ),
};

// --- SFX ---------------------------------------------------------------

export interface SfxFile {
  name: string;
  path: string;
  size_bytes: number;
  modified_at: string;
  referenced: boolean;
}

export interface SfxFolder {
  name: string;
  path: string;
  file_count: number;
}

export interface SfxTreeResponse {
  path: string;
  folders: SfxFolder[];
  files: SfxFile[];
}

export interface SfxUploadResult {
  saved: SfxFile[];
  destination: string;
}

export const sfxApi = {
  allFiles: () => api.get<SfxFile[]>("/api/sfx/files"),
  tree: (path = "") =>
    api.get<SfxTreeResponse>(
      path ? `/api/sfx/tree?path=${encodeURIComponent(path)}` : "/api/sfx/tree",
    ),
  upload: (
    files: File[],
    dest: string,
    onProgress?: (loaded: number, total: number) => void,
  ): Promise<SfxUploadResult> => {
    const form = new FormData();
    for (const f of files) form.append("files", f, f.name);
    return new Promise<SfxUploadResult>((resolve, reject) => {
      const xhr = new XMLHttpRequest();
      const path = `/api/sfx/upload?dest=${encodeURIComponent(dest)}`;
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
            resolve(JSON.parse(xhr.responseText) as SfxUploadResult);
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
  moveFile: (src: string, dstFolder: string, newFilename?: string) =>
    api.post<SfxFile>("/api/sfx/move", {
      src,
      dst_folder: dstFolder,
      new_filename: newFilename,
    }),
  deleteFile: (path: string) =>
    api.delete<void>(`/api/sfx/files?path=${encodeURIComponent(path)}`),
  createFolder: (path: string) =>
    api.post<SfxFolder>("/api/sfx/folders", { path }),
  deleteFolder: (path: string, recursive: boolean) => {
    const qs = new URLSearchParams({ path, recursive: String(recursive) });
    return api.delete<{ deleted: boolean }>(`/api/sfx/folders?${qs.toString()}`);
  },
  renameFolder: (src: string, dst: string) =>
    api.post<SfxFolder>("/api/sfx/folders/rename", { src, dst }),
  fileUrl: (path: string) =>
    `${BASE}/api/sfx/file?path=${encodeURIComponent(path)}`,
};

// --- modes scaffolding -------------------------------------------------

export const modesAdminApi = {
  create: (id: string, name: string) =>
    api.post<ModeSummary>("/api/modes", { id, name }),
  delete: (id: string) =>
    api.delete<void>(`/api/modes/${encodeURIComponent(id)}`),
  reload: () =>
    api.post<{ loaded: string[]; errors: Record<string, string> }>(
      "/api/modes/reload",
    ),
  createSoundboard: (modeId: string, payload: { id: string; name?: string }) =>
    api.post<unknown>(
      `/api/modes/${encodeURIComponent(modeId)}/soundboards`,
      payload,
    ),
  deleteSoundboard: (modeId: string, soundboardId: string) =>
    api.delete<void>(
      `/api/modes/${encodeURIComponent(modeId)}/soundboards/${encodeURIComponent(soundboardId)}`,
    ),
  createScene: (
    modeId: string,
    payload: { id: string; name: string; description?: string },
  ) =>
    api.post<unknown>(
      `/api/modes/${encodeURIComponent(modeId)}/scenes`,
      payload,
    ),
  deleteScene: (modeId: string, sceneId: string) =>
    api.delete<void>(
      `/api/modes/${encodeURIComponent(modeId)}/scenes/${encodeURIComponent(sceneId)}`,
    ),
  updateScene: (
    modeId: string,
    sceneId: string,
    payload: {
      name?: string;
      description?: string;
      ambient?: { playlist?: string; crossfade_ms?: number };
      clear_ambient?: boolean;
      presets?: string[];
      looping_sfx?: SceneLoopingSfx[];
    },
  ) =>
    api.patch<SceneSpec>(
      `/api/modes/${encodeURIComponent(modeId)}/scenes/${encodeURIComponent(sceneId)}`,
      payload,
    ),

  // Soundboard editor — categories + items inside an existing soundboard.
  // Each call returns the updated SoundboardManifest so the UI can re-render
  // without a separate fetch.
  addCategory: (
    modeId: string,
    soundboardId: string,
    payload: { id: string; name: string },
  ) =>
    api.post<{
      id: string;
      name?: string | null;
      categories: Array<{
        id: string;
        name: string;
        items: Array<{ file: string; name: string; hotkey?: string | null; icon?: string | null }>;
      }>;
    }>(
      `/api/modes/${encodeURIComponent(modeId)}/soundboards/${encodeURIComponent(soundboardId)}/categories`,
      payload,
    ),
  deleteCategory: (modeId: string, soundboardId: string, categoryId: string) =>
    api.delete<unknown>(
      `/api/modes/${encodeURIComponent(modeId)}/soundboards/${encodeURIComponent(soundboardId)}/categories/${encodeURIComponent(categoryId)}`,
    ),
  addItem: (
    modeId: string,
    soundboardId: string,
    categoryId: string,
    payload: { file: string; name: string; hotkey?: string; icon?: string },
  ) =>
    api.post<unknown>(
      `/api/modes/${encodeURIComponent(modeId)}/soundboards/${encodeURIComponent(soundboardId)}/categories/${encodeURIComponent(categoryId)}/items`,
      payload,
    ),
  updateItem: (
    modeId: string,
    soundboardId: string,
    categoryId: string,
    index: number,
    payload: { name?: string; hotkey?: string; icon?: string; file?: string },
  ) =>
    api.patch<unknown>(
      `/api/modes/${encodeURIComponent(modeId)}/soundboards/${encodeURIComponent(soundboardId)}/categories/${encodeURIComponent(categoryId)}/items/${index}`,
      payload,
    ),
  deleteItem: (
    modeId: string,
    soundboardId: string,
    categoryId: string,
    index: number,
  ) =>
    api.delete<unknown>(
      `/api/modes/${encodeURIComponent(modeId)}/soundboards/${encodeURIComponent(soundboardId)}/categories/${encodeURIComponent(categoryId)}/items/${index}`,
    ),

  // Interrupt templates — saved on the mode's manifest.yaml.
  addInterrupt: (
    modeId: string,
    payload: {
      name: string;
      playlist?: string;
      soundboard_item?: string;
      fade_in_ms?: number;
      fade_out_ms?: number;
      return_to_ambient?: boolean;
    },
  ) =>
    api.post<InterruptSpec[]>(
      `/api/modes/${encodeURIComponent(modeId)}/interrupts`,
      payload,
    ),
  updateInterrupt: (
    modeId: string,
    index: number,
    payload: Partial<{
      name: string;
      playlist: string | null;
      soundboard_item: string | null;
      fade_in_ms: number;
      fade_out_ms: number;
      return_to_ambient: boolean;
    }>,
  ) =>
    api.patch<InterruptSpec[]>(
      `/api/modes/${encodeURIComponent(modeId)}/interrupts/${index}`,
      payload,
    ),
  deleteInterrupt: (modeId: string, index: number) =>
    api.delete<InterruptSpec[]>(
      `/api/modes/${encodeURIComponent(modeId)}/interrupts/${index}`,
    ),
};

// --- presets scaffolding -----------------------------------------------

export const presetsAdminApi = {
  create: (payload: {
    id: string;
    name: string;
    description?: string;
    effects: PresetEffect[];
  }) => api.post<PresetManifest>("/api/presets", payload),
  update: (
    id: string,
    payload: { name?: string; description?: string; effects?: PresetEffect[] },
  ) => api.put<PresetManifest>(`/api/presets/${encodeURIComponent(id)}`, payload),
  delete: (id: string) =>
    api.delete<void>(`/api/presets/${encodeURIComponent(id)}`),
  reload: () =>
    api.post<{ loaded: string[]; errors: Record<string, string> }>(
      "/api/presets/reload",
    ),
};

// Re-export types we already had so callers don't need to dig in /core/types.
export type { FolderEntry };
