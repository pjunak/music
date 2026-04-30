import { useEffect, useState } from "react";

import { libraryApi } from "@/core/api";
import { selectActiveTrackId, usePlayerStore } from "@/core/playerStore";
import { trackTitle } from "@/core/trackDisplay";
import type { Track } from "@/core/types";
import { useUiStore } from "@/core/uiStore";

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
          <h1 className="player-title">{trackTitle(track)}</h1>
          <p className="player-artist">
            {track.artist || "(unknown artist)"}
            {track.album ? ` — ${track.album}` : ""}
            {track.origin ? ` · from ${track.origin}` : ""}
          </p>

          {queueTracks.length > 0 ? (
            <section className="player-queue">
              <h3>Up next</h3>
              <ol>
                {queueTracks.map((t) => (
                  <li key={t.id}>
                    <span className="track-title">{trackTitle(t)}</span>
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
                    {trackTitle(t)}
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

