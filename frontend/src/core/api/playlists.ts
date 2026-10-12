import type { AutomaticPlaylistApplyResult, AutomaticPlaylistPreview, AutomaticPlaylistRule, PlaylistMeta, TrackInPlaylist } from "@/core/types";

import { BASE, api } from "./http";

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
  previewAutomatic: (playlistId: number, rule: AutomaticPlaylistRule) =>
    api.post<AutomaticPlaylistPreview>(
      `/api/playlists/${playlistId}/automatic/preview`,
      { rule },
    ),
  configureAutomatic: (
    playlistId: number,
    rule: AutomaticPlaylistRule,
    sourceSignature: string,
  ) =>
    api.put<AutomaticPlaylistApplyResult>(
      `/api/playlists/${playlistId}/automatic`,
      { rule, source_signature: sourceSignature },
    ),
  refreshAutomatic: (playlistId: number) =>
    api.post<AutomaticPlaylistApplyResult>(
      `/api/playlists/${playlistId}/automatic/refresh`,
    ),
  disableAutomatic: (playlistId: number) =>
    api.delete<PlaylistMeta>(`/api/playlists/${playlistId}/automatic`),
  exportUrl: (playlistId: number, format: "m3u" | "json") =>
    `${BASE}/api/playlists/${playlistId}/export?format=${format}`,
};
