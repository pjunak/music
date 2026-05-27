import { useEffect, useState } from "react";
import { Link } from "react-router-dom";

import { IconButton } from "@/components/IconButton";
import { PlayIcon } from "@/components/icons";
import { diagnosticsApi, playlistsApi } from "@/core/api";
import { selectActiveTrackId, usePlayerStore } from "@/core/playerStore";
import type { PlaylistMeta } from "@/core/types";
import { wsClient } from "@/core/ws";

import { InterruptSection } from "./controls/InterruptSection";
import { PresetsSection } from "./controls/PresetsSection";
import { ScenesSection } from "./controls/ScenesSection";
import { SoundboardSection } from "./controls/SoundboardSection";
import { TransportSection } from "./controls/TransportSection";

/** The Console tab is the DM's *live* workspace. Authoring (creating
 *  playlists, editing modes/scenes/presets, managing files) lives in the
 *  dedicated tabs. Anything in here should be a thing the DM does mid-
 *  session: fire SFX, switch scenes, fire interrupts, etc.
 *
 *  Mode picker moved to the header (reachable from any tab); master volume
 *  and per-this-device output toggle live in the persistent NowPlayingBar.
 *  What's left here is the multi-device output picker (which TVs are
 *  currently outputting) — that's still a live concern, so it stays on
 *  this tab as a slim bar above the action grid. */
export function ControlsView() {
  return (
    <div className="controls-view">
      <FirstRunWelcome />
      <OutputsBar />
      <div className="controls-grid">
        <section className="surface-card span-tall">
          <h3>Scenes</h3>
          <ScenesSection />
        </section>
        <section className="surface-card span-tall">
          <h3>Soundboard</h3>
          <SoundboardSection />
        </section>
        <section className="surface-card">
          <h3>Transport</h3>
          <TransportSection />
        </section>
        <section className="surface-card">
          <h3>Interrupt</h3>
          <InterruptSection />
        </section>
        <section className="surface-card">
          <h3>EQ Presets</h3>
          <PresetsSection />
        </section>
        <section className="surface-card">
          <h3>Quick-play playlists</h3>
          <QuickPlaylists />
        </section>
      </div>
    </div>
  );
}

// --- first-run welcome card -------------------------------------------
//
// When the indexed-track count is zero, surface a friendly nudge toward
// the Library tab. Without this the operator sees an empty Scenes grid /
// empty Quick-play list and may wonder if something's broken, when really
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
        Your library has 0 tracks indexed. Drop audio files into{" "}
        <strong>Library → Files</strong> and they'll show up in scenes,
        quick-play playlists, and the soundboard.
      </p>
      <div>
        <Link to="/library/files" className="btn-link">
          <PlayIcon />
          Go to Library
        </Link>
      </div>
    </section>
  );
}

// --- outputs bar: which connected devices are currently outputting audio?
//
// Distinct from the NowPlayingBar's OutputToggle pill — that one is "is
// THIS device an output?". This bar is the multi-device picker: "is the
// living-room TV an output? what about the bedroom TV?" The operator may
// want to fan out audio to multiple rooms mid-session.

function OutputsBar() {
  const state = usePlayerStore((s) => s.state);
  const myDeviceId = usePlayerStore((s) => s.myDeviceId);
  const devices = state?.connected_devices ?? [];
  const activeIds = state?.active_output_device_ids ?? [];

  const audioOutputs = devices.filter((d) =>
    d.capabilities.includes("audio_output"),
  );

  function toggle(deviceId: string) {
    const next = activeIds.includes(deviceId)
      ? activeIds.filter((d) => d !== deviceId)
      : [...activeIds, deviceId];
    wsClient.send({ type: "set_active_outputs", device_ids: next });
  }

  return (
    <div className="outputs-bar" role="group" aria-label="Active audio outputs">
      <span className="outputs-bar-label">Outputs</span>
      {audioOutputs.length === 0 ? (
        <span className="muted small">
          No audio-output devices connected. Open this page on a TV / speaker
          tab and enable Audio output in Settings.
        </span>
      ) : (
        <div className="output-picker-options inline">
          {audioOutputs.map((d) => {
            const checked = activeIds.includes(d.device_id);
            const isMe = d.device_id === myDeviceId;
            return (
              <label key={d.device_id} className="output-picker-option">
                <input
                  type="checkbox"
                  checked={checked}
                  onChange={() => toggle(d.device_id)}
                />
                <span>
                  {d.name}
                  {isMe ? " (this)" : ""}
                </span>
              </label>
            );
          })}
        </div>
      )}
    </div>
  );
}

// --- quick-play playlists: just a flat list of "play this whole playlist now"

function QuickPlaylists() {
  const activeModeId = usePlayerStore((s) => s.state?.active_mode_id ?? null);
  const activeTrackId = usePlayerStore(selectActiveTrackId);
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

  // Active track id only used to flag if any of these are currently driving.
  void activeTrackId;

  return (
    <ul className="quick-playlist-list">
      {playlists.map((p) => (
        <li key={p.id}>
          <span className="playlist-name">{p.name}</span>
          <span className="muted small">
            {p.category ? `${p.category} · ` : ""}
            {p.mode_id ?? "global"}
          </span>
          <IconButton
            label="Play this playlist now"
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
      ))}
    </ul>
  );
}
