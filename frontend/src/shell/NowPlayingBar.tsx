import { useEffect, useRef, useState } from "react";
import type { MouseEvent } from "react";

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
  const progressRef = useRef<HTMLDivElement | null>(null);

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
        const t = await libraryApi.getTrack(displayId);
        if (!cancelled) setTrack(t);
      } catch {
        if (!cancelled) setTrack(null);
      }
    })();
    return () => {
      cancelled = true;
    };
  }, [displayId]);

  // Tick the position display so it advances visually while playing.
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

  function onSeek(e: MouseEvent<HTMLDivElement>) {
    if (totalMs <= 0) return;
    const el = progressRef.current;
    if (el === null) return;
    const rect = el.getBoundingClientRect();
    const fraction = Math.max(0, Math.min(1, (e.clientX - rect.left) / rect.width));
    const seekMs = Math.round(fraction * totalMs);
    if (interruptId !== null) {
      wsClient.send({ type: "interrupt_seek", position_ms: seekMs });
    } else {
      wsClient.send({ type: "ambient_seek", position_ms: seekMs });
    }
  }

  const seekable = totalMs > 0;
  const fraction = seekable ? Math.min(1, positionMs / totalMs) : 0;

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
        <button onClick={prev} title="Previous">⏮</button>
        {isPlaying ? (
          <button onClick={pause} title="Pause">⏸</button>
        ) : (
          <button onClick={play} title="Play">▶</button>
        )}
        <button onClick={next} title="Next">⏭</button>
      </div>
      <div className="now-playing-position">
        <span>{formatTime(positionMs)}</span>
        <div
          ref={progressRef}
          className={`seek-bar${seekable ? "" : " seek-bar-disabled"}`}
          onClick={seekable ? onSeek : undefined}
          role={seekable ? "slider" : undefined}
          aria-label={seekable ? "Seek" : undefined}
          aria-valuemin={0}
          aria-valuemax={totalMs}
          aria-valuenow={Math.min(positionMs, totalMs)}
          tabIndex={seekable ? 0 : -1}
          title={seekable ? "Click to seek" : undefined}
        >
          <div
            className="seek-bar-fill"
            style={{ width: `${(fraction * 100).toFixed(2)}%` }}
          />
        </div>
        <span>{seekable ? formatTime(totalMs) : "—"}</span>
      </div>
    </footer>
  );
}
