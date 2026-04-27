import { useCallback, useEffect, useState } from "react";
import type { FormEvent } from "react";

import { libraryApi } from "@/core/api";
import type { Track } from "@/core/types";
import { wsClient } from "@/core/ws";
import { UploadManager } from "@/panels/UploadManager";

export function LibraryPanel() {
  const [query, setQuery] = useState("");
  const [tracks, setTracks] = useState<Track[]>([]);
  const [loading, setLoading] = useState(false);
  const [error, setError] = useState<string | null>(null);

  const runSearch = useCallback(async (q: string) => {
    setLoading(true);
    setError(null);
    try {
      const res = await libraryApi.search(q);
      setTracks(res.tracks);
    } catch (e) {
      setError(e instanceof Error ? e.message : "search failed");
      setTracks([]);
    } finally {
      setLoading(false);
    }
  }, []);

  useEffect(() => {
    // Initial load — first 50 tracks from the library.
    void runSearch("");
  }, [runSearch]);

  function onSubmit(e: FormEvent) {
    e.preventDefault();
    void runSearch(query);
  }

  function play(beetsId: number) {
    wsClient.send({ type: "ambient_play_track", beets_id: beetsId });
  }

  function enqueue(beetsId: number) {
    wsClient.send({ type: "ambient_enqueue", beets_id: beetsId });
  }

  return (
    <section className="panel">
      <h2>Library</h2>
      <UploadManager onIngestComplete={() => void runSearch(query)} />
      <form onSubmit={onSubmit} className="library-search">
        <input
          type="search"
          value={query}
          onChange={(e) => setQuery(e.target.value)}
          placeholder='Beets query — e.g. artist:daft year:2001..'
        />
        <button type="submit" disabled={loading}>
          {loading ? "…" : "Search"}
        </button>
      </form>
      {error !== null ? <p className="error small">{error}</p> : null}
      {tracks.length === 0 && !loading ? (
        <p className="muted small">No tracks. Add to your Beets library and reload.</p>
      ) : (
        <ul className="track-list">
          {tracks.map((t) => (
            <li key={t.beets_id} className="track-list-item">
              <div className="track-list-meta">
                <span className="track-title">{t.title || "(untitled)"}</span>
                <span className="muted small">
                  {t.artist || "(unknown artist)"}
                  {t.album ? ` · ${t.album}` : ""}
                </span>
              </div>
              <div className="track-list-actions">
                <button onClick={() => play(t.beets_id)}>Play</button>
                <button onClick={() => enqueue(t.beets_id)}>Queue</button>
              </div>
            </li>
          ))}
        </ul>
      )}
    </section>
  );
}
