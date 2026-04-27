import { useEffect, useState } from "react";

import { playlistsApi } from "@/core/api";
import { usePlayerStore } from "@/core/playerStore";
import type { PlaylistMeta } from "@/core/types";
import { wsClient } from "@/core/ws";

export function PlaylistsPanel() {
  const activeModeId = usePlayerStore((s) => s.state?.active_mode_id ?? null);
  const [playlists, setPlaylists] = useState<PlaylistMeta[]>([]);
  const [error, setError] = useState<string | null>(null);

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

  function play(playlistId: number) {
    wsClient.send({ type: "ambient_play_playlist", playlist_id: playlistId });
  }

  return (
    <section className="panel">
      <h2>Playlists</h2>
      {error !== null ? <p className="error small">{error}</p> : null}
      {playlists.length === 0 ? (
        <p className="muted small">
          {activeModeId !== null
            ? `No playlists for mode "${activeModeId}" or global.`
            : "No playlists. Pick a mode to filter, or create one."}
        </p>
      ) : (
        <ul className="playlist-list">
          {playlists.map((p) => (
            <li key={p.id} className="playlist-list-item">
              <div className="playlist-list-item-meta">
                <span className="playlist-name">{p.name}</span>
                <span className="muted small">
                  {p.source}
                  {p.category !== null ? ` · ${p.category}` : ""}
                  {p.mode_id !== null ? ` · ${p.mode_id}` : " · global"}
                </span>
              </div>
              <button onClick={() => play(p.id)}>Play</button>
            </li>
          ))}
        </ul>
      )}
    </section>
  );
}
