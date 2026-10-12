import type * as Http from "@/generated/http";

import { BASE, api, uploadWithProgress } from "./http";
import type { UploadCheckItem, UploadConflict } from "./library";

// --- SFX ---------------------------------------------------------------

export type SfxFile = Http.SfxFileOut;

export type SfxFolder = Http.SfxFolderOut;

// Files immediately in one folder; the hierarchy comes from /api/sfx/folders.
export type SfxTreeResponse = Http.SfxTreeResponse;

export type SfxFoldersResponse = Http.SfxFoldersResponse;

export type SfxUploadResult = Http.SfxUploadResult;

export const sfxApi = {
  allFiles: () => api.get<SfxFile[]>("/api/sfx/files"),
  tree: (path = "") =>
    api.get<SfxTreeResponse>(
      path ? `/api/sfx/tree?path=${encodeURIComponent(path)}` : "/api/sfx/tree",
    ),
  allFolders: () => api.get<SfxFoldersResponse>("/api/sfx/folders"),
  upload: (
    files: File[],
    dest: string,
    onProgress?: (loaded: number, total: number) => void,
    conflict?: UploadConflict,
  ): Promise<SfxUploadResult> =>
    uploadWithProgress<SfxUploadResult>(
      `/api/sfx/upload?dest=${encodeURIComponent(dest)}${
        conflict ? `&conflict=${conflict}` : ""
      }`,
      files,
      onProgress,
    ),
  checkUpload: (items: UploadCheckItem[]) =>
    api.post<{ collisions: UploadCheckItem[] }>("/api/sfx/upload/check", { items }),
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
