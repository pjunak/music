import { useEffect, useState } from "react";
import { Link } from "react-router-dom";

import { IconButton } from "@/components/IconButton";
import { PauseIcon, PlayIcon } from "@/components/icons";
import { diagnosticsApi, playlistsApi } from "@/core/api";
import { usePlayerStore } from "@/core/playerStore";
import type { PlaylistMeta } from "@/core/types";
import { wsClient } from "@/core/ws";

import { CuesSection } from "./controls/CuesSection";
import { InterruptSection } from "./controls/InterruptSection";
import { LoopsSection } from "./controls/LoopsSection";
import { PresetsSection } from "./controls/PresetsSection";
import { SoundboardSection } from "./controls/SoundboardSection";
import { TransportSection } from "./controls/TransportSection";

/** The Console tab is the DM's *live* workspace. Authoring (creating
 *  playlists, editing modes/presets/cues, managing files) lives in the
 *  dedicated tabs. Anything in here should be a thing the DM does mid-
 *  session: fire SFX, fire cues, fire interrupts, etc.
 *
 *  Mode picker moved to the header (reachable from any tab); master volume
 *  and the **Speakers** control (which devices output + per-device volume)
 *  live in the persistent NowPlayingBar footer. This tab is just the live
 *  action grid. */
export function ControlsView() {
  return (
    <div className="controls-view">
      <h1 className="sr-only">Console</h1>
      <FirstRunWelcome />
      <div className="controls-grid">
        <section className="surface-card span-tall" aria-labelledby="panel-cues-h">
          <h3 id="panel-cues-h">Cues</h3>
          <CuesSection />
        </section>
        <section className="surface-card span-tall" aria-labelledby="panel-soundboard-h">
          <h3 id="panel-soundboard-h">Soundboard</h3>
          <SoundboardSection />
        </section>
        <section className="surface-card" aria-labelledby="panel-transport-h">
          <h3 id="panel-transport-h">Transport</h3>
          <TransportSection />
        </section>
        <section className="surface-card" aria-labelledby="panel-interrupt-h">
          <h3 id="panel-interrupt-h">Interrupt</h3>
          <InterruptSection />
        </section>
        <section className="surface-card" aria-labelledby="panel-loops-h">
          <h3 id="panel-loops-h">Loops</h3>
          <LoopsSection />
        </section>
        <section className="surface-card" aria-labelledby="panel-presets-h">
          <h3 id="panel-presets-h">EQ Presets</h3>
          <PresetsSection />
        </section>
        <section className="surface-card" aria-labelledby="panel-playlists-h">
          <h3 id="panel-playlists-h">Quick-play playlists</h3>
          <QuickPlaylists />
        </section>
      </div>
    </div>
  );
}

// --- first-run welcome card -------------------------------------------
//
// When the indexed-track count is zero, surface a friendly nudge toward
// the Library tab. Without this the operator sees an empty Quick-play list /
// empty soundboard and may wonder if something's broken, when really
// the answer is "we haven't uploaded anything yet."
//
// One-shot fetch on mount; if the count is non-zero the card never
// renders, so the normal Console layout is unchanged for everyone past
// their first session.

function FirstRunWelcome() {
  const [trackCount, setTrackCount] = useState<number | null>(null);

  useEffect(() => {
    let cancelled = false;
    void diagnosticsApi
      .get()
      .then((d) => {
        if (!cancelled) setTrackCount(d.track_count);
      })
      .catch(() => undefined);
    return () => {
      cancelled = true;
    };
  }, []);

  if (trackCount === null || trackCount > 0) return null;

  return (
    <section className="surface-card first-run-welcome">
      <h2>Welcome — let's get some music in.</h2>
      <p className="muted small">
        Your library has 0 tracks indexed. Drop audio files into the{" "}
        <strong>Library</strong> tab and they'll show up in cues, quick-play
        playlists, and the soundboard.
      </p>
      <div>
        <Link to="/library" className="btn-link">
          <PlayIcon />
          Go to Library
        </Link>
      </div>
    </section>
  );
}

// --- quick-play playlists: just a flat list of "play this whole playlist now"

function QuickPlaylists() {
  const activeModeId = usePlayerStore((s) => s.state?.active_mode_id ?? null);
  // Which playlist is currently driving the ambient lane (server-tracked).
  const drivingId = usePlayerStore(
    (s) => s.state?.ambient.source_playlist_id ?? null,
  );
  const isPlaying = usePlayerStore((s) => s.state?.is_playing ?? false);
  const [playlists, setPlaylists] = useState<PlaylistMeta[]>([]);

  useEffect(() => {
    playlistsApi
      .list(activeModeId !== null ? { mode_id: activeModeId } : {})
      .then(setPlaylists)
      .catch(() => setPlaylists([]));
  }, [activeModeId]);

  if (playlists.length === 0) {
    return (
      <p className="muted small">
        No playlists{activeModeId !== null ? ` for "${activeModeId}"` : ""}. Create
        one in the Playlists tab.
      </p>
    );
  }

  return (
    <ul className="quick-playlist-list">
      {playlists.map((p) => {
        const driving = p.id === drivingId;
        return (
          <li key={p.id} className={driving ? "driving" : undefined}>
            <span className="playlist-name">
              {driving ? (
                <span className="driving-badge" title="Now driving ambient" aria-hidden="true">
                  {isPlaying ? <PlayIcon /> : <PauseIcon />}
                </span>
              ) : null}
              {p.name}
            </span>
            <span className="muted small">{p.category || "Playlist"}</span>
            <IconButton
              label={driving ? "Restart this playlist" : "Play this playlist now"}
              icon={<PlayIcon />}
              variant="primary"
              onClick={() =>
                wsClient.send({
                  type: "ambient_play_playlist",
                  playlist_id: p.id,
                })
              }
            />
          </li>
        );
      })}
    </ul>
  );
}
