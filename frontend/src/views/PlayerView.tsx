import { useEffect, useState } from "react";

import { libraryApi } from "@/core/api";
import { useAuthStore } from "@/core/auth";
import {
  selectActiveTrackId,
  selectIsMyOutput,
  usePlayerStore,
} from "@/core/playerStore";
import type { Track } from "@/core/types";
import { useUiStore } from "@/core/uiStore";
import { wsClient } from "@/core/ws";

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
 *  the Player view.
 *
 *  - Logged-in user: claim/release via the server (set_active_outputs).
 *  - Guest (no login): a local-only "Play here" toggle flips the playback
 *    engine into "treat me as an output" mode, since guests can't mutate
 *    server state. Other clients won't see this device in the Outputs
 *    picker — the audio is purely local. */
function OutputBadge() {
  const isMyOutput = usePlayerStore(selectIsMyOutput);
  const myDeviceId = usePlayerStore((s) => s.myDeviceId);
  const activeIds = usePlayerStore(
    (s) => s.state?.active_output_device_ids,
  );
  const me = usePlayerStore((s) =>
    s.state?.connected_devices.find((d) => d.device_id === s.myDeviceId) ?? null,
  );
  const authStatus = useAuthStore((s) => s.status);
  const isGuest = authStatus !== "authenticated";
  const forceLocal = useUiStore((s) => s.forceLocalPlayback);
  const setForceLocal = useUiStore((s) => s.setForceLocalPlayback);

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

  // Guest: server won't accept set_active_outputs from us, so flip the
  // local-only playback flag instead. The audio engine respects it the
  // same way it respects the server's active_output_device_ids.
  if (isGuest) {
    if (forceLocal) {
      return (
        <div className="output-badge output-badge-active">
          <span className="badge badge-ok">🔊 Playing on this device (local)</span>
          <button
            type="button"
            className="btn-ghost"
            onClick={() => setForceLocal(false)}
          >
            Stop playing here
          </button>
        </div>
      );
    }
    return (
      <div className="output-badge output-badge-inactive">
        <span className="badge badge-warn">🔇 Not playing here</span>
        <button
          type="button"
          className="btn-primary"
          onClick={() => setForceLocal(true)}
        >
          Play on this device
        </button>
        <span className="muted small">
          Local-only — sign in to share this device with the operator.
        </span>
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
