import { useEffect, useMemo, useRef, useState } from "react";
import type { ChangeEvent, MouseEvent } from "react";

import { libraryApi } from "@/core/api";
import {
  selectActiveTrackId,
  selectAmbientPositionMs,
  selectIsMyOutput,
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
  // Subscribe directly to the array reference (or `undefined` when the WS
  // hasn't sent a snapshot yet). Don't `?? []` inside the selector — that
  // creates a fresh array literal on every render, which makes a downstream
  // `useEffect([queueIds])` see a "changed" dep on every render and loops
  // until React error #185 fires.
  const queueIds = usePlayerStore((s) => s.state?.ambient.queue);
  const historyIds = usePlayerStore((s) => s.state?.ambient.history);
  const positionMs = usePlayerStore(selectAmbientPositionMs);
  // Stable string keys derived from the id arrays — re-runs the fetch
  // effects only when the ids that matter actually change, not on every
  // unrelated state_changed broadcast.
  const queueKey = (queueIds ?? []).slice(0, 3).join("|");
  const historyKey = (historyIds ?? []).slice(-2).join("|");
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
    if (queueKey === "") {
      setQueueTracks([]);
      return;
    }
    const ids = queueKey.split("|").map(Number);
    void Promise.all(ids.map((id) => libraryApi.getTrack(id).catch(() => null))).then(
      (rs) => {
        if (!cancelled) setQueueTracks(rs.filter((t): t is Track => t !== null));
      },
    );
    return () => {
      cancelled = true;
    };
  }, [queueKey]);

  useEffect(() => {
    let cancelled = false;
    if (historyKey === "") {
      setHistoryTracks([]);
      return;
    }
    // Most-recent-first.
    const ids = historyKey.split("|").map(Number).reverse();
    void Promise.all(ids.map((id) => libraryApi.getTrack(id).catch(() => null))).then(
      (rs) => {
        if (!cancelled) setHistoryTracks(rs.filter((t): t is Track => t !== null));
      },
    );
    return () => {
      cancelled = true;
    };
  }, [historyKey]);

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
        <OutputBadge />
        <div className="player-empty-art" aria-hidden="true">
          ♪
        </div>
        <h1 className="player-title">Nothing playing</h1>
        <p className="muted">
          Open <strong>Library</strong> and press ▶ on a track, or pick a
          playlist from <strong>Controls</strong>.
        </p>
        <PlayerVolume />
      </div>
    );
  }

  return (
    <div className="player-view player-view-active">
      <OutputBadge />
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

          <PlayerVolume />

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
                {(queueIds?.length ?? 0) > queueTracks.length ? (
                  <li className="muted small">
                    +{(queueIds?.length ?? 0) - queueTracks.length} more
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

/** Always-visible "is this device the speaker?" indicator at the top of
 *  the Player view. Green pill when yes, neutral pill with a one-click
 *  "Make this device the speaker" button when no. Useful on a TV bookmark
 *  where the operator opens the page and isn't sure if it's actually
 *  going to play audio. */
function OutputBadge() {
  const isMyOutput = usePlayerStore(selectIsMyOutput);
  const myDeviceId = usePlayerStore((s) => s.myDeviceId);
  const activeIds = usePlayerStore(
    (s) => s.state?.active_output_device_ids,
  );
  const me = usePlayerStore((s) =>
    s.state?.connected_devices.find((d) => d.device_id === s.myDeviceId) ?? null,
  );

  function claim() {
    if (myDeviceId === null) return;
    const next = activeIds === undefined ? [myDeviceId] : [...activeIds, myDeviceId];
    wsClient.send({ type: "set_active_outputs", device_ids: next });
  }

  function release() {
    if (myDeviceId === null || activeIds === undefined) return;
    wsClient.send({
      type: "set_active_outputs",
      device_ids: activeIds.filter((d) => d !== myDeviceId),
    });
  }

  if (myDeviceId === null) {
    return (
      <div className="output-badge output-badge-disconnected">
        <span className="badge">⏳ Connecting…</span>
      </div>
    );
  }

  if (me === null || !me.capabilities.includes("audio_output")) {
    return (
      <div className="output-badge output-badge-disabled">
        <span className="badge">🔇 This device isn't an audio output</span>
        <span className="muted small">
          Enable in Settings → This device → Audio output
        </span>
      </div>
    );
  }

  if (isMyOutput) {
    return (
      <div className="output-badge output-badge-active">
        <span className="badge badge-ok">🔊 Playing on this device</span>
        <button type="button" className="btn-ghost" onClick={release}>
          Stop playing here
        </button>
      </div>
    );
  }

  return (
    <div className="output-badge output-badge-inactive">
      <span className="badge badge-warn">🔇 Not playing here</span>
      <button type="button" className="btn-primary" onClick={claim}>
        Play on this device
      </button>
    </div>
  );
}

/** Volume slider on the Player tab. Mirrors the master volume from
 *  PlayerState; sending `set_volume` updates the server, which broadcasts
 *  back to all clients (including this one). */
function PlayerVolume() {
  const volume = usePlayerStore((s) => s.state?.volume ?? 1.0);
  function onChange(e: ChangeEvent<HTMLInputElement>) {
    wsClient.send({ type: "set_volume", volume: parseFloat(e.target.value) });
  }
  return (
    <label className="player-volume">
      <span className="muted small">Volume</span>
      <input
        type="range"
        min={0}
        max={1}
        step={0.01}
        value={volume}
        onChange={onChange}
        aria-label="Master volume"
      />
      <span className="muted small player-volume-pct">
        {Math.round(volume * 100)}%
      </span>
    </label>
  );
}
