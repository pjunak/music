import type {
  Cue,
  FolderEntry,
  InterruptSpec,
  KnownDevice,
  ModeDetail,
  ModeSummary,
  PlaylistMeta,
  PresetEffect,
  PresetManifest,
  SoundboardManifest,
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

// XHR (not fetch) because only XHR exposes upload-progress events.
function uploadWithProgress<T>(
  path: string,
  files: File[],
  onProgress?: (loaded: number, total: number) => void,
): Promise<T> {
  const form = new FormData();
  for (const f of files) form.append("files", f, f.name);
  return new Promise<T>((resolve, reject) => {
    const xhr = new XMLHttpRequest();
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
          resolve(JSON.parse(xhr.responseText) as T);
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
}

// --- typed helpers per resource -----------------------------------------

export const modesApi = {
  list: () => api.get<ModeSummary[]>("/api/modes"),
  get: (id: string) => api.get<ModeDetail>(`/api/modes/${encodeURIComponent(id)}`),
};

export interface ActiveSession {
  token_prefix: string;
  created_at: string;
  expires_at: string;
  last_seen: string;
  is_current: boolean;
}

export const authApi = {
  listSessions: () => api.get<ActiveSession[]>("/api/auth/sessions"),
  revokeSession: (tokenPrefix: string) =>
    api.delete<void>(
      `/api/auth/sessions/${encodeURIComponent(tokenPrefix)}`,
    ),
};

export type { PresetManifest, PresetEffect } from "@/core/types";

export const presetsApi = {
  // Presets are per-mode now — list the given mode's EQ presets.
  list: (modeId: string) =>
    api.get<PresetManifest[]>(
      `/api/modes/${encodeURIComponent(modeId)}/presets`,
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
  update: (
    playlistId: number,
    payload: { name: string; mode_id?: string | null; category?: string | null },
  ) => api.patch<PlaylistMeta>(`/api/playlists/${playlistId}`, payload),
  exportUrl: (playlistId: number, format: "m3u" | "json") =>
    `${BASE}/api/playlists/${playlistId}/export?format=${format}`,
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
  disc_no?: number | null;
  year?: number | null;
  genre?: string;
  bpm?: number | null;
  // DB-only fields — not written to the file's tags. See backend.
  display_title?: string;
  origin?: string;
}

export interface BulkMetadataUpdate {
  track_ids: number[];
  updates: MetadataUpdate;
}

export interface BulkMetadataSkip {
  track_id: number;
  reason: string;
}

export interface BulkMetadataResult {
  updated: Track[];
  skipped: BulkMetadataSkip[];
}

export interface BulkActionSkip {
  track_id: number;
  reason: string;
}

export interface BulkMoveResult {
  moved: Track[];
  skipped: BulkActionSkip[];
}

export interface BulkDeleteResult {
  deleted_ids: number[];
  skipped: BulkActionSkip[];
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
  ): Promise<UploadResult> =>
    uploadWithProgress<UploadResult>(
      `/api/library/upload?dest=${encodeURIComponent(dest)}`,
      files,
      onProgress,
    ),
  rescan: () => api.post<RescanResult>("/api/library/rescan"),
  updateMetadata: (id: number, payload: MetadataUpdate) =>
    api.patch<Track>(`/api/library/tracks/${id}/metadata`, payload),
  updateBulkMetadata: (payload: BulkMetadataUpdate) =>
    api.patch<BulkMetadataResult>("/api/library/tracks/bulk-metadata", payload),
  moveTrack: (id: number, destination: string, newFilename?: string) =>
    api.post<Track>(`/api/library/tracks/${id}/move`, {
      destination,
      new_filename: newFilename,
    }),
  deleteTrack: (id: number) => api.delete<void>(`/api/library/tracks/${id}`),
  bulkMove: (trackIds: number[], destination: string) =>
    api.post<BulkMoveResult>("/api/library/tracks/bulk-move", {
      track_ids: trackIds,
      destination,
    }),
  bulkDelete: (trackIds: number[]) =>
    api.post<BulkDeleteResult>("/api/library/tracks/bulk-delete", {
      track_ids: trackIds,
    }),
  coverUrl: (id: number) => `${BASE}/api/library/tracks/${id}/cover`,
  streamUrl: (id: number) => `${BASE}/api/library/tracks/${id}/stream`,
  createFolder: (path: string) =>
    api.post<{ name: string; path: string; track_count: number; has_children: boolean }>(
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
    api.post<{ name: string; path: string; track_count: number; has_children: boolean }>(
      "/api/library/folders/rename",
      { src, dst },
    ),
};

// --- library cleanup -----------------------------------------------------

// Keep in lockstep with the backend `RuleId` Literal in app/api/cleanup.py
// and the engine rule constants in app/library/cleanup.py.
export type CleanupRuleId =
  | "strip_track_numbers"
  | "strip_artist"
  | "strip_album"
  | "strip_junk"
  | "normalize_separators"
  | "normalize_case"
  | "tag_title"
  | "tag_artist"
  | "tag_album"
  | "tag_number"
  | "tag_year";

export interface CleanupScope {
  type: "all" | "folder" | "tracks";
  path?: string;
  recursive?: boolean;
  track_ids?: number[];
}

export interface CleanupOp {
  op_id: string;
  track_id: number;
  kind: "rename" | "tag";
  field: string | null;
  old: string | number | null;
  new: string | number | null;
  rules: string[];
  confidence: "high" | "low";
  /** Value confirmed by an online name lookup (MusicBrainz). */
  verified: boolean;
}

export interface CleanupTrackPlan {
  track_id: number;
  path: string;
  ops: CleanupOp[];
  notes: string[];
}

export interface CleanupAnalyzeResult {
  scanned: number;
  plans: CleanupTrackPlan[];
  /** Names an online lookup could still settle — resolve via
   *  cleanupApi.verify, then re-analyze (verdicts are cached forever,
   *  each name is only ever looked up once). */
  pending_lookups: string[];
}

export interface CleanupVerifyResult {
  verified: number;
  failed: string[];
}

export interface CleanupOpIn {
  track_id: number;
  kind: "rename" | "tag";
  field: string | null;
  old: string | number | null;
  new: string | number | null;
}

export interface CleanupApplyResult {
  batch_id: number | null;
  applied: number;
  skipped: BulkActionSkip[];
}

export interface CleanupBatchSummary {
  id: number;
  created_at: string;
  scope_label: string;
  item_count: number;
  reverted_at: string | null;
}

export interface CleanupBatchDetail extends CleanupBatchSummary {
  items: unknown[];
}

export interface CleanupRevertResult {
  reverted: number;
  skipped: BulkActionSkip[];
}

export const cleanupApi = {
  analyze: (scope: CleanupScope, rules: CleanupRuleId[]) =>
    api.post<CleanupAnalyzeResult>("/api/library/cleanup/analyze", { scope, rules }),
  /** Resolve a small batch of names against MusicBrainz (server paces at
   *  1 req/s — keep batches ≤ 5 and chunk longer lists). */
  verify: (names: string[]) =>
    api.post<CleanupVerifyResult>("/api/library/cleanup/verify", { names }),
  /** One chunk of accepted ops. Pass the batch_id from the previous chunk
   *  so the whole run lands in a single revertable journal. */
  apply: (ops: CleanupOpIn[], batchId: number | null, scopeLabel: string) =>
    api.post<CleanupApplyResult>("/api/library/cleanup/apply", {
      ops,
      batch_id: batchId,
      scope_label: scopeLabel,
    }),
  batches: () => api.get<CleanupBatchSummary[]>("/api/library/cleanup/batches"),
  batch: (id: number) =>
    api.get<CleanupBatchDetail>(`/api/library/cleanup/batches/${id}`),
  revertBatch: (id: number) =>
    api.post<CleanupRevertResult>(`/api/library/cleanup/batches/${id}/revert`),
  /** Revert from a previously-downloaded journal file (disaster path —
   *  works even after the server-side batch rows are gone). */
  revertJournal: (items: unknown[]) =>
    api.post<CleanupRevertResult>("/api/library/cleanup/revert", { items }),
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
  has_children: boolean;
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
  ): Promise<SfxUploadResult> =>
    uploadWithProgress<SfxUploadResult>(
      `/api/sfx/upload?dest=${encodeURIComponent(dest)}`,
      files,
      onProgress,
    ),
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
  rename: (id: string, name: string) =>
    api.patch<ModeSummary>(`/api/modes/${encodeURIComponent(id)}`, { name }),
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
  // Soundboard editor — categories + items inside an existing soundboard.
  // Each call returns the updated SoundboardManifest so the UI can re-render
  // without a separate fetch.
  addCategory: (
    modeId: string,
    soundboardId: string,
    payload: { id: string; name: string },
  ) =>
    api.post<SoundboardManifest>(
      `/api/modes/${encodeURIComponent(modeId)}/soundboards/${encodeURIComponent(soundboardId)}/categories`,
      payload,
    ),
  deleteCategory: (modeId: string, soundboardId: string, categoryId: string) =>
    api.delete<SoundboardManifest>(
      `/api/modes/${encodeURIComponent(modeId)}/soundboards/${encodeURIComponent(soundboardId)}/categories/${encodeURIComponent(categoryId)}`,
    ),
  addItem: (
    modeId: string,
    soundboardId: string,
    categoryId: string,
    payload: { file: string; name: string; hotkey?: string; icon?: string },
  ) =>
    api.post<SoundboardManifest>(
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
    api.patch<SoundboardManifest>(
      `/api/modes/${encodeURIComponent(modeId)}/soundboards/${encodeURIComponent(soundboardId)}/categories/${encodeURIComponent(categoryId)}/items/${index}`,
      payload,
    ),
  deleteItem: (
    modeId: string,
    soundboardId: string,
    categoryId: string,
    index: number,
  ) =>
    api.delete<SoundboardManifest>(
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
      duck_to?: number | null;
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
      duck_to: number | null;
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

  createCue: (modeId: string, payload: Cue) =>
    api.post<Cue>(`/api/modes/${encodeURIComponent(modeId)}/cues`, payload),
  updateCue: (modeId: string, cueId: string, payload: Omit<Cue, "id">) =>
    api.put<Cue>(
      `/api/modes/${encodeURIComponent(modeId)}/cues/${encodeURIComponent(cueId)}`,
      payload,
    ),
  deleteCue: (modeId: string, cueId: string) =>
    api.delete<void>(
      `/api/modes/${encodeURIComponent(modeId)}/cues/${encodeURIComponent(cueId)}`,
    ),
};

// --- presets scaffolding -----------------------------------------------

// EQ presets are per-mode now — CRUD lives under the modes API.
export const presetsAdminApi = {
  create: (
    modeId: string,
    payload: {
      id: string;
      name: string;
      description?: string;
      effects: PresetEffect[];
      volume?: number | null;
      crossfade_ms?: number | null;
    },
  ) =>
    api.post<PresetManifest>(
      `/api/modes/${encodeURIComponent(modeId)}/presets`,
      payload,
    ),
  update: (
    modeId: string,
    presetId: string,
    payload: {
      name: string;
      description?: string;
      effects?: PresetEffect[];
      volume?: number | null;
      crossfade_ms?: number | null;
    },
  ) =>
    api.put<PresetManifest>(
      `/api/modes/${encodeURIComponent(modeId)}/presets/${encodeURIComponent(presetId)}`,
      payload,
    ),
  delete: (modeId: string, presetId: string) =>
    api.delete<void>(
      `/api/modes/${encodeURIComponent(modeId)}/presets/${encodeURIComponent(presetId)}`,
    ),
};

// Diagnostics — server-side operational snapshot. Read by the
// Diagnostics tab so the operator can see what's happening (track
// count, last rescan, mode load errors — a per-mode preset error is
// folded into that mode's error string — connected devices) without
// SSH'ing into the host.
export interface LoaderStatus {
  last_load_at: number | null;
  loaded_ids: string[];
  errors: Record<string, string>;
}

export interface DiagnosticsResponse {
  track_count: number;
  last_scan_at: number | null;
  modes: LoaderStatus;
  connected_device_count: number;
  state_revision: number;
}

export const diagnosticsApi = {
  get: () => api.get<DiagnosticsResponse>("/api/diagnostics"),
};

// Re-export types we already had so callers don't need to dig in /core/types.
export type { FolderEntry };
