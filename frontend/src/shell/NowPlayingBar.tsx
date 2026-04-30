import { useEffect, useRef, useState } from "react";
import type { ChangeEvent, MouseEvent } from "react";

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

/** Persistent footer.
 *
 *  Three rows on the inside, but visually a single bar:
 *    1. Track meta (title / artist) on the left, transport buttons centred,
 *       volume on the right.
 *    2. A wide, easy-to-grab scrub bar with elapsed/total clocks bracketing
 *       it. The scrub bar is the headline interactive element here — taller
 *       than the inline volume slider so the two read as different controls.
 *
 *  Since this bar is mounted on every route, every shortcut path through
 *  the app has playback / volume / seek without having to leave the
 *  current view. */
export function NowPlayingBar() {
  const isPlaying = usePlayerStore((s) => s.state?.is_playing ?? false);
  const currentId = usePlayerStore((s) => s.state?.ambient.current_track_id ?? null);
  const interruptId = usePlayerStore(
    (s) => s.state?.interrupt?.current_track_id ?? null,
  );
  const volume = usePlayerStore((s) => s.state?.volume ?? 1);

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

  function onVolumeChange(e: ChangeEvent<HTMLInputElement>) {
    wsClient.send({ type: "set_volume", volume: parseFloat(e.target.value) });
  }

  const seekable = totalMs > 0;
  const fraction = seekable ? Math.min(1, positionMs / totalMs) : 0;

  return (
    <footer className="now-playing">
      <div className="now-playing-top">
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
          <button onClick={prev} title="Previous (←)" aria-label="Previous">⏮</button>
          {isPlaying ? (
            <button
              onClick={pause}
              title="Pause (Space)"
              aria-label="Pause"
              className="now-playing-play"
            >
              ⏸
            </button>
          ) : (
            <button
              onClick={play}
              title="Play (Space)"
              aria-label="Play"
              className="now-playing-play"
            >
              ▶
            </button>
          )}
          <button onClick={next} title="Next (→)" aria-label="Next">⏭</button>
        </div>

        <label className="now-playing-volume" title="Volume">
          <span aria-hidden="true">🔉</span>
          <input
            className="volume-slider"
            type="range"
            min={0}
            max={1}
            step={0.01}
            value={volume}
            onChange={onVolumeChange}
            aria-label="Master volume"
          />
          <span className="now-playing-volume-pct">
            {Math.round(volume * 100)}%
          </span>
        </label>
      </div>

      <div className="now-playing-scrub">
        <span className="now-playing-clock">{formatTime(positionMs)}</span>
        <div
          ref={progressRef}
          className={`seek-bar seek-bar-large${seekable ? "" : " seek-bar-disabled"}`}
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
          <div
            className="seek-bar-thumb"
            style={{ left: `${(fraction * 100).toFixed(2)}%` }}
            aria-hidden="true"
          />
        </div>
        <span className="now-playing-clock">
          {seekable ? formatTime(totalMs) : "—"}
        </span>
      </div>
    </footer>
  );
}
