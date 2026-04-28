import { useEffect, useState } from "react";

import { playlistsApi } from "@/core/api";
import { usePlayerStore } from "@/core/playerStore";
import type { PlaylistMeta, TrackInPlaylist } from "@/core/types";
import { wsClient } from "@/core/ws";

export function PlaylistsPanel() {
  const activeModeId = usePlayerStore((s) => s.state?.active_mode_id ?? null);
  const ambientCurrentId = usePlayerStore(
    (s) => s.state?.ambient.current_beets_id ?? null,
  );
  const [playlists, setPlaylists] = useState<PlaylistMeta[]>([]);
  const [error, setError] = useState<string | null>(null);
  const [expanded, setExpanded] = useState<number | null>(null);
  const [tracksByPlaylist, setTracksByPlaylist] = useState<
    Record<number, TrackInPlaylist[] | "loading" | { error: string }>
  >({});

  useEffect(() => {
    setError(null);
    playlistsApi
      .list(activeModeId !== null ? { mode_id: activeModeId } : {})
      .then(setPlaylists)
      .catch((e: unknown) => {
        setError(e instanceof Error ? e.message : "load failed");
        setPlaylists([]);
      });
  }, [activeModeId]);

  async function toggleExpand(playlistId: number) {
    if (expanded === playlistId) {
      setExpanded(null);
      return;
    }
    setExpanded(playlistId);
    if (tracksByPlaylist[playlistId] !== undefined) return;
    setTracksByPlaylist((prev) => ({ ...prev, [playlistId]: "loading" }));
    try {
      const tracks = await playlistsApi.tracks(playlistId);
      setTracksByPlaylist((prev) => ({ ...prev, [playlistId]: tracks }));
    } catch (e) {
      const detail = e instanceof Error ? e.message : "load failed";
      setTracksByPlaylist((prev) => ({ ...prev, [playlistId]: { error: detail } }));
    }
  }

  function play(playlistId: number) {
    wsClient.send({ type: "ambient_play_playlist", playlist_id: playlistId });
  }

  function enqueueTrack(beetsId: number) {
    wsClient.send({ type: "ambient_enqueue", beets_id: beetsId });
  }

  function playTrack(beetsId: number) {
    wsClient.send({ type: "ambient_play_track", beets_id: beetsId });
  }

  return (
    <section className="panel">
      <h2>Playlists</h2>
      {error !== null ? <p className="error small">{error}</p> : null}
      {playlists.length === 0 ? (
        <p className="muted small">
          {activeModeId !== null
            ? `No playlists for mode "${activeModeId}" or global.`
            : "No playlists yet."}
        </p>
      ) : (
        <ul className="playlist-list">
          {playlists.map((p) => {
            const isOpen = expanded === p.id;
            const tracks = tracksByPlaylist[p.id];
            return (
              <li key={p.id} className="playlist-list-item-wrap">
                <div className="playlist-list-item">
                  <button
                    type="button"
                    className="playlist-disclosure"
                    onClick={() => void toggleExpand(p.id)}
                    title={isOpen ? "Collapse" : "Expand to view tracks"}
                  >
                    {isOpen ? "▾" : "▸"}
                  </button>
                  <div className="playlist-list-item-meta">
                    <span className="playlist-name">{p.name}</span>
                    <span className="muted small">
                      {p.source}
                      {p.category !== null ? ` · ${p.category}` : ""}
                      {p.mode_id !== null ? ` · ${p.mode_id}` : " · global"}
                    </span>
                  </div>
                  <button
                    type="button"
                    onClick={() => play(p.id)}
                    title="Replace ambient lane with this playlist"
                  >
                    Play
                  </button>
                </div>
                {isOpen ? (
                  <div className="playlist-tracks">
                    {tracks === "loading" ? (
                      <p className="muted small">Loading tracks…</p>
                    ) : tracks === undefined ? null : "error" in tracks ? (
                      <p className="error small">{tracks.error}</p>
                    ) : tracks.length === 0 ? (
                      <p className="muted small">Empty playlist.</p>
                    ) : (
                      <ol className="playlist-track-list">
                        {tracks.map((t) => {
                          const isPlaying = ambientCurrentId === t.beets_id;
                          const label =
                            t.display_name ||
                            t.track?.title ||
                            `Track ${t.beets_id}`;
                          const subtitle = t.track
                            ? `${t.track.artist}${
                                t.track.album ? ` · ${t.track.album}` : ""
                              }`
                            : null;
                          return (
                            <li
                              key={`${t.position}-${t.beets_id}`}
                              className={`playlist-track ${
                                isPlaying ? "playing" : ""
                              }`}
                            >
                              <span className="playlist-track-pos muted small">
                                {t.position + 1}
                              </span>
                              <div className="playlist-track-meta">
                                <span className="playlist-track-title">{label}</span>
                                {subtitle !== null ? (
                                  <span className="muted small">{subtitle}</span>
                                ) : null}
                              </div>
                              <div className="playlist-track-actions">
                                <button
                                  type="button"
                                  onClick={() => playTrack(t.beets_id)}
                                  title="Play this track now"
                                >
                                  ▶
                                </button>
                                <button
                                  type="button"
                                  onClick={() => enqueueTrack(t.beets_id)}
                                  title="Add to ambient queue"
                                >
                                  ＋
                                </button>
                              </div>
                            </li>
                          );
                        })}
                      </ol>
                    )}
                  </div>
                ) : null}
              </li>
            );
          })}
        </ul>
      )}
    </section>
  );
}
