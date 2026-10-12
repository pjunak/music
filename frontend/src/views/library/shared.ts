import type { TreeFolder } from "@/components/FolderTree";
import { libraryApi, sfxApi } from "@/core/api";

export type Root = "music" | "sfx";

/** Stable key for an upload target. JSON-encoding the (dest, name) pair is
 *  unambiguous — a space (or any char) in either half can't be mistaken for
 *  a separator — so collision lookups never false-match across folders. */
export function collisionKey(dest: string, name: string): string {
  return JSON.stringify([dest, name]);
}

export function formatSize(bytes: number): string {
  if (bytes < 1024) return `${bytes} B`;
  if (bytes < 1024 * 1024) return `${(bytes / 1024).toFixed(1)} KB`;
  if (bytes < 1024 * 1024 * 1024) return `${(bytes / (1024 * 1024)).toFixed(1)} MB`;
  return `${(bytes / (1024 * 1024 * 1024)).toFixed(2)} GB`;
}

export function formatDuration(seconds: number): string {
  if (!seconds || seconds <= 0) return "—";
  const total = Math.floor(seconds);
  return `${Math.floor(total / 60)}:${(total % 60).toString().padStart(2, "0")}`;
}

export const musicAllFolders = async (): Promise<TreeFolder[]> =>
  (await libraryApi.allFolders()).folders.map((f) => ({
    name: f.name,
    path: f.path,
    badge: f.track_count > 0 ? f.track_count : null,
  }));

export const sfxAllFolders = async (): Promise<TreeFolder[]> =>
  (await sfxApi.allFolders()).folders.map((f) => ({
    name: f.name,
    path: f.path,
    badge: f.file_count > 0 ? f.file_count : null,
  }));

export function basename(p: string): string {
  return p.split("/").pop() || p;
}
