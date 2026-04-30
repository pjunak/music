import { useEffect, useState } from "react";

import { IconButton } from "@/components/IconButton";
import { PlayIcon } from "@/components/icons";
import { VolumeControl } from "@/components/VolumeControl";
import { modesApi, playlistsApi } from "@/core/api";
import { selectActiveTrackId, usePlayerStore } from "@/core/playerStore";
import type { ModeSummary, PlaylistMeta } from "@/core/types";
import { wsClient } from "@/core/ws";

import { InterruptSection } from "./controls/InterruptSection";
import { PresetsSection } from "./controls/PresetsSection";
import { ScenesSection } from "./controls/ScenesSection";
import { SoundboardSection } from "./controls/SoundboardSection";
import { TransportSection } from "./controls/TransportSection";

/** The Controls tab is the DM's *live* surface. Authoring (creating
 *  playlists, editing modes/scenes/presets, managing files) lives in the
 *  dedicated tabs. Anything in here should be a thing the DM does mid-
 *  session: pick a mode, fire SFX, switch scenes, tweak volume, etc. */
export function ControlsView() {
  return (
    <div className="controls-view">
      <ContextStrip />
      <div className="controls-grid">
        <section className="controls-section span-tall">
          <h3>Scenes</h3>
          <ScenesSection />
        </section>
        <section className="controls-section span-tall">
          <h3>Soundboard</h3>
          <SoundboardSection />
        </section>
        <section className="controls-section">
          <h3>Transport</h3>
          <TransportSection />
        </section>
        <section className="controls-section">
          <h3>Interrupt</h3>
          <InterruptSection />
        </section>
        <section className="controls-section">
          <h3>Presets</h3>
          <PresetsSection />
        </section>
        <section className="controls-section">
          <h3>Quick-play playlists</h3>
          <QuickPlaylists />
        </section>
      </div>
      <footer className="controls-footer">
        <a
          href="/diagnostics"
          target="_blank"
          rel="noopener noreferrer"
          className="btn-ghost"
        >
          🔧 Open diagnostics in new tab
        </a>
      </footer>
    </div>
  );
}

// --- always-visible top strip: mode + outputs + master volume ----------

function ContextStrip() {
  return (
    <div className="controls-strip">
      <div className="controls-strip-section">
        <span className="controls-strip-label">Mode</span>
        <ModeSelect />
      </div>
      <div className="controls-strip-section">
        <span className="controls-strip-label">Outputs</span>
        <OutputToggles />
      </div>
      <div className="controls-strip-section">
        <span className="controls-strip-label">Volume</span>
        <VolumeSlider />
      </div>
    </div>
  );
}

function ModeSelect() {
  const [modes, setModes] = useState<ModeSummary[]>([]);
  const activeModeId = usePlayerStore((s) => s.state?.active_mode_id ?? null);

  useEffect(() => {
    modesApi.list().then(setModes).catch(() => setModes([]));
  }, []);

  return (
    <select
      value={activeModeId ?? ""}
      onChange={(e) =>
        wsClient.send({
          type: "set_active_mode",
          mode_id: e.target.value === "" ? null : e.target.value,
        })
      }
    >
      <option value="">— none —</option>
      {modes.map((m) => (
        <option key={m.id} value={m.id}>
          {m.name}
        </option>
      ))}
    </select>
  );
}

function OutputToggles() {
  const state = usePlayerStore((s) => s.state);
  const myDeviceId = usePlayerStore((s) => s.myDeviceId);
  const devices = state?.connected_devices ?? [];
  const activeIds = state?.active_output_device_ids ?? [];

  const audioOutputs = devices.filter((d) => d.capabilities.includes("audio_output"));

  if (audioOutputs.length === 0) {
    return <span className="muted small">none registered</span>;
  }

  function toggle(deviceId: string) {
    const next = activeIds.includes(deviceId)
      ? activeIds.filter((d) => d !== deviceId)
      : [...activeIds, deviceId];
    wsClient.send({ type: "set_active_outputs", device_ids: next });
  }

  return (
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
  );
}

function VolumeSlider() {
  const volume = usePlayerStore((s) => s.state?.volume ?? 1.0);
  return (
    <VolumeControl
      value={volume}
      onChange={(next) => wsClient.send({ type: "set_volume", volume: next })}
      label="Master volume"
      showIcon={false}
    />
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
