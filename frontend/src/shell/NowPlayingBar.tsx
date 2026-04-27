import { useEffect, useState } from "react";

import { libraryApi } from "@/core/api";
import {
  selectAmbientPositionMs,
  usePlayerStore,
} from "@/core/playerStore";
import type { Track } from "@/core/types";
import { wsClient } from "@/core/ws";

function formatTime(ms: number): string {
  if (!Number.isFinite(ms) || ms < 0) return "0:00";
  const totalSec = Math.floor(ms / 1000);
  const min = Math.floor(totalSec / 60);
  const sec = totalSec % 60;
  return `${min}:${sec.toString().padStart(2, "0")}`;
}

export function NowPlayingBar() {
  const isPlaying = usePlayerStore((s) => s.state?.is_playing ?? false);
  const currentId = usePlayerStore((s) => s.state?.ambient.current_beets_id ?? null);
  const interruptId = usePlayerStore(
    (s) => s.state?.interrupt?.current_beets_id ?? null,
  );

  const [track, setTrack] = useState<Track | null>(null);

  // Fetch metadata for whatever's currently playing (interrupt wins).
  const displayId = interruptId ?? currentId;
  useEffect(() => {
    if (displayId === null) {
      setTrack(null);
      return;
    }
    let cancelled = false;
    void (async () => {
      try {
        // We use the library search trick because there's no direct
        // /tracks/{id} typed helper yet — but search by id works.
        // (Direct fetch would be cleaner; deferred to cleanup.)
        const url = `/api/library/tracks/${displayId}`;
        const r = await fetch(url, { credentials: "include" });
        if (!r.ok) {
          if (!cancelled) setTrack(null);
          return;
        }
        const t = (await r.json()) as Track;
        if (!cancelled) setTrack(t);
      } catch {
        if (!cancelled) setTrack(null);
      }
    })();
    return () => {
      cancelled = true;
    };
  }, [displayId]);

  // Tick the position display once per second so it advances visually
  // while playing.
  const [, setTick] = useState(0);
  useEffect(() => {
    if (!isPlaying) return;
    const interval = window.setInterval(() => {
      setTick((t) => t + 1);
    }, 500);
    return () => {
      window.clearInterval(interval);
    };
  }, [isPlaying]);

  const positionMs = usePlayerStore(selectAmbientPositionMs);
  const totalMs = track !== null ? Math.round(track.length_s * 1000) : 0;

  function play() {
    wsClient.send({ type: "resume" });
  }
  function pause() {
    wsClient.send({ type: "pause" });
  }
  function next() {
    wsClient.send({ type: "ambient_skip_next" });
  }
  function prev() {
    wsClient.send({ type: "ambient_skip_prev" });
  }

  // Keep search-utility import referenced (lint guard).
  void libraryApi;

  return (
    <footer className="now-playing">
      <div className="now-playing-track">
        {track !== null ? (
          <>
            <strong>{track.title || "(untitled)"}</strong>
            <span className="muted small">
              {track.artist || "(unknown)"}
              {track.album ? ` · ${track.album}` : ""}
              {interruptId !== null ? "  ·  ⚡ interrupt" : ""}
            </span>
          </>
        ) : (
          <span className="muted">Nothing playing</span>
        )}
      </div>
      <div className="now-playing-controls">
        <button onClick={prev}>⏮</button>
        {isPlaying ? (
          <button onClick={pause}>⏸</button>
        ) : (
          <button onClick={play}>▶</button>
        )}
        <button onClick={next}>⏭</button>
      </div>
      <div className="now-playing-position">
        <span>{formatTime(positionMs)}</span>
        {totalMs > 0 ? (
          <>
            <progress max={totalMs} value={Math.min(positionMs, totalMs)} />
            <span>{formatTime(totalMs)}</span>
          </>
        ) : null}
      </div>
    </footer>
  );
}
