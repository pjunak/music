import { useEffect, useMemo, useRef, useState } from "react";
import type { MouseEvent } from "react";

import { libraryApi } from "@/core/api";
import {
  selectActiveTrackId,
  selectAmbientPositionMs,
  usePlayerStore,
} from "@/core/playerStore";
import type { Track } from "@/core/types";
import { useUiStore } from "@/core/uiStore";
import { wsClient } from "@/core/ws";

function formatClock(ms: number): string {
  if (!Number.isFinite(ms) || ms < 0) return "0:00";
  const total = Math.floor(ms / 1000);
  const m = Math.floor(total / 60);
  const s = total % 60;
  return `${m}:${s.toString().padStart(2, "0")}`;
}

export function PlayerView() {
  const trackId = usePlayerStore(selectActiveTrackId);
  const isPlaying = usePlayerStore((s) => s.state?.is_playing ?? false);
  const interruptActive = usePlayerStore((s) => s.state?.interrupt !== null);
  const queueIds = usePlayerStore((s) => s.state?.ambient.queue ?? []);
  const historyIds = usePlayerStore((s) => s.state?.ambient.history ?? []);
  const positionMs = usePlayerStore(selectAmbientPositionMs);
  const hidePlayerArt = useUiStore((s) => s.hidePlayerArt);

  const [track, setTrack] = useState<Track | null>(null);
  const [coverFailed, setCoverFailed] = useState(false);
  const [queueTracks, setQueueTracks] = useState<Track[]>([]);
  const [historyTracks, setHistoryTracks] = useState<Track[]>([]);
  const seekRef = useRef<HTMLDivElement | null>(null);

  useEffect(() => {
    if (trackId === null) {
      setTrack(null);
      return;
    }
    let cancelled = false;
    setCoverFailed(false);
    void libraryApi
      .getTrack(trackId)
      .then((t) => {
        if (!cancelled) setTrack(t);
      })
      .catch(() => {
        if (!cancelled) setTrack(null);
      });
    return () => {
      cancelled = true;
    };
  }, [trackId]);

  // Fetch metadata for the next 3 in queue and most recent 2 in history.
  useEffect(() => {
    let cancelled = false;
    const upcoming = queueIds.slice(0, 3);
    Promise.all(upcoming.map((id) => libraryApi.getTrack(id).catch(() => null)))
      .then((rs) => {
        if (!cancelled) setQueueTracks(rs.filter((t): t is Track => t !== null));
      });
    return () => {
      cancelled = true;
    };
  }, [queueIds]);

  useEffect(() => {
    let cancelled = false;
    const recent = historyIds.slice(-2).reverse();
    Promise.all(recent.map((id) => libraryApi.getTrack(id).catch(() => null)))
      .then((rs) => {
        if (!cancelled) setHistoryTracks(rs.filter((t): t is Track => t !== null));
      });
    return () => {
      cancelled = true;
    };
  }, [historyIds]);

  // Tick once per 500ms while playing so the position display advances.
  const [, setTick] = useState(0);
  useEffect(() => {
    if (!isPlaying || interruptActive) return;
    const t = window.setInterval(() => setTick((n) => n + 1), 500);
    return () => window.clearInterval(t);
  }, [isPlaying, interruptActive]);

  const totalMs = useMemo(
    () => (track !== null ? Math.round(track.length_s * 1000) : 0),
    [track],
  );
  const seekable = totalMs > 0;
  const fraction = seekable ? Math.min(1, positionMs / totalMs) : 0;

  function onSeek(e: MouseEvent<HTMLDivElement>) {
    if (!seekable || seekRef.current === null) return;
    const rect = seekRef.current.getBoundingClientRect();
    const f = Math.max(0, Math.min(1, (e.clientX - rect.left) / rect.width));
    wsClient.send({ type: "ambient_seek", position_ms: Math.round(f * totalMs) });
  }

  if (hidePlayerArt) {
    // Blackout mode for a room display: no art, no chrome. The persistent
    // NowPlayingBar at the bottom still gives controls if needed.
    return <div className="player-view player-view-blackout" aria-hidden="true" />;
  }

  if (track === null) {
    return (
      <div className="player-view player-view-empty">
        <div className="player-empty-art" aria-hidden="true">
          ♪
        </div>
        <h1 className="player-title">Nothing playing</h1>
        <p className="muted">
          Open <strong>Library</strong> and press ▶ on a track, or pick a
          playlist from <strong>Controls</strong>.
        </p>
      </div>
    );
  }

  return (
    <div className="player-view player-view-active">
      <div className="player-stage">
        <div className="player-art player-art-large">
          {!coverFailed ? (
            <img
              src={libraryApi.coverUrl(track.id)}
              alt=""
              onError={() => setCoverFailed(true)}
            />
          ) : (
            <div className="player-art-placeholder">♪</div>
          )}
        </div>
        <div className="player-meta">
          <p className="player-status">
            {interruptActive ? (
              <span className="badge badge-warn">⚡ Interrupt</span>
            ) : isPlaying ? (
              <span className="badge badge-ok">▶ Playing</span>
            ) : (
              <span className="badge">⏸ Paused</span>
            )}
          </p>
          <h1 className="player-title">{track.title || track.path}</h1>
          <p className="player-artist">
            {track.artist || "(unknown artist)"}
            {track.album ? ` — ${track.album}` : ""}
          </p>

          <div className="player-seek-row">
            <span className="player-clock">{formatClock(positionMs)}</span>
            <div
              ref={seekRef}
              className={`seek-bar large${seekable ? "" : " seek-bar-disabled"}`}
              onClick={seekable ? onSeek : undefined}
              role={seekable ? "slider" : undefined}
              aria-valuemin={0}
              aria-valuemax={totalMs}
              aria-valuenow={Math.min(positionMs, totalMs)}
              tabIndex={seekable ? 0 : -1}
            >
              <div
                className="seek-bar-fill"
                style={{ width: `${(fraction * 100).toFixed(2)}%` }}
              />
            </div>
            <span className="player-clock">
              {seekable ? formatClock(totalMs) : "—"}
            </span>
          </div>

          {queueTracks.length > 0 ? (
            <section className="player-queue">
              <h3>Up next</h3>
              <ol>
                {queueTracks.map((t) => (
                  <li key={t.id}>
                    <span className="track-title">{t.title || t.path}</span>
                    {t.artist ? (
                      <span className="muted small"> · {t.artist}</span>
                    ) : null}
                  </li>
                ))}
                {queueIds.length > queueTracks.length ? (
                  <li className="muted small">
                    +{queueIds.length - queueTracks.length} more
                  </li>
                ) : null}
              </ol>
            </section>
          ) : null}

          {historyTracks.length > 0 ? (
            <section className="player-history">
              <h3>Recently played</h3>
              <ol>
                {historyTracks.map((t) => (
                  <li key={t.id} className="muted small">
                    {t.title || t.path}
                    {t.artist ? ` · ${t.artist}` : ""}
                  </li>
                ))}
              </ol>
            </section>
          ) : null}
        </div>
      </div>
    </div>
  );
}
