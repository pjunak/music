import type { FoldersResponse, Track, TreeResponse } from "@/core/types";
import type * as Http from "@/generated/http";

import { BASE, api, uploadWithProgress } from "./http";

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

export type SearchResponse = Http.SearchResponse;

export type UploadResult = Http.UploadResult;

/** How an upload handles a filename that already exists at the destination.
 *  "rename" keeps both (foo.mp3 -> foo-1.mp3), "overwrite" replaces, "skip"
 *  leaves the existing file untouched. */
export type UploadConflict = "rename" | "overwrite" | "skip";

export type UploadCheckItem = Http.UploadCheckItem;

export type RescanResult = Http.RescanResult;

export type MetadataUpdate = Http.TrackMetadataUpdate;

export type BulkMetadataUpdate = Http.BulkMetadataUpdate;

export type BulkMetadataSkip = Http.BulkMetadataSkip;

export type BulkMetadataResult = Http.BulkMetadataResult;

export type BulkActionSkip = Http.BulkActionSkip;

export type BulkMoveResult = Http.BulkMoveResult;

export type BulkDeleteResult = Http.BulkDeleteResult;

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
  allFolders: () => api.get<FoldersResponse>("/api/library/folders"),
  upload: (
    files: File[],
    dest: string,
    onProgress?: (loaded: number, total: number) => void,
    conflict?: UploadConflict,
  ): Promise<UploadResult> =>
    uploadWithProgress<UploadResult>(
      `/api/library/upload?dest=${encodeURIComponent(dest)}${
        conflict ? `&conflict=${conflict}` : ""
      }`,
      files,
      onProgress,
    ),
  /** Preflight: which of these (dest, name) targets already exist on disk. */
  checkUpload: (items: UploadCheckItem[]) =>
    api.post<{ collisions: UploadCheckItem[] }>("/api/library/upload/check", {
      items,
    }),
  rescan: () => api.post<RescanResult>("/api/library/rescan"),
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

// Keep in lockstep with the Rust cleanup rules and generated API contract.
