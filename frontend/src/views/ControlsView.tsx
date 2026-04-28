import { useEffect, useState } from "react";

import { modesApi, playlistsApi } from "@/core/api";
import { selectActiveTrackId, usePlayerStore } from "@/core/playerStore";
import type { ModeSummary, PlaylistMeta, TrackInPlaylist } from "@/core/types";
import { wsClient } from "@/core/ws";

import { InterruptSection } from "./controls/InterruptSection";
import { PresetsSection } from "./controls/PresetsSection";
import { ScenesSection } from "./controls/ScenesSection";
import { SoundboardSection } from "./controls/SoundboardSection";
import { TransportSection } from "./controls/TransportSection";

export function ControlsView() {
  // Two-column layout on wide screens. Left = "context / state-changing
  // settings the DM doesn't touch every minute." Right = "active surface
  // the DM hits during play (scenes, soundboard)."
  return (
    <div className="controls-view">
      <div className="controls-grid">
        <div className="controls-col">
          <section className="controls-section">
            <h3>Mode</h3>
            <ModeSection />
          </section>
          <section className="controls-section">
            <h3>Outputs</h3>
            <OutputSection />
          </section>
          <section className="controls-section">
            <h3>Volume</h3>
            <VolumeSection />
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
        </div>
        <div className="controls-col">
          <section className="controls-section">
            <h3>Scenes</h3>
            <ScenesSection />
          </section>
          <section className="controls-section">
            <h3>Soundboard</h3>
            <SoundboardSection />
          </section>
          <section className="controls-section">
            <h3>Playlists</h3>
            <PlaylistsSection />
          </section>
        </div>
      </div>
    </div>
  );
}

// --- Mode picker ----------------------------------------------------------

function ModeSection() {
  const [modes, setModes] = useState<ModeSummary[]>([]);
  const activeModeId = usePlayerStore((s) => s.state?.active_mode_id ?? null);

  useEffect(() => {
    modesApi.list().then(setModes).catch(() => setModes([]));
  }, []);

  function onChange(value: string) {
    wsClient.send({
      type: "set_active_mode",
      mode_id: value === "" ? null : value,
    });
  }

  return (
    <label className="mode-picker">
      <select value={activeModeId ?? ""} onChange={(e) => onChange(e.target.value)}>
        <option value="">— none —</option>
        {modes.map((m) => (
          <option key={m.id} value={m.id}>
            {m.name}
          </option>
        ))}
      </select>
    </label>
  );
}

// --- Output picker --------------------------------------------------------

function OutputSection() {
  const state = usePlayerStore((s) => s.state);
  const myDeviceId = usePlayerStore((s) => s.myDeviceId);
  const devices = state?.connected_devices ?? [];
  const activeIds = state?.active_output_device_ids ?? [];

  const audioOutputs = devices.filter((d) => d.capabilities.includes("audio_output"));

  function toggle(deviceId: string) {
    const next = activeIds.includes(deviceId)
      ? activeIds.filter((d) => d !== deviceId)
      : [...activeIds, deviceId];
    wsClient.send({ type: "set_active_outputs", device_ids: next });
  }

  if (audioOutputs.length === 0) {
    return <p className="muted small">No audio outputs registered yet.</p>;
  }

  return (
    <div className="output-picker-options">
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

// --- Volume slider --------------------------------------------------------

function VolumeSection() {
  const volume = usePlayerStore((s) => s.state?.volume ?? 1.0);
  return (
    <label className="volume-slider">
      <input
        type="range"
        min={0}
        max={1}
        step={0.01}
        value={volume}
        onChange={(e) =>
          wsClient.send({ type: "set_volume", volume: parseFloat(e.target.value) })
        }
      />
      <span>{Math.round(volume * 100)}%</span>
    </label>
  );
}

// --- Playlists ------------------------------------------------------------

function PlaylistsSection() {
  const activeModeId = usePlayerStore((s) => s.state?.active_mode_id ?? null);
  const activeTrackId = usePlayerStore(selectActiveTrackId);
  const [playlists, setPlaylists] = useState<PlaylistMeta[]>([]);
  const [error, setError] = useState<string | null>(null);
  const [expanded, setExpanded] = useState<number | null>(null);
  const [tracks, setTracks] = useState<
    Record<number, TrackInPlaylist[] | "loading" | { error: string }>
  >({});

  useEffect(() => {
    setError(null);
    playlistsApi
      .list(activeModeId !== null ? { mode_id: activeModeId } : {})
      .then(setPlaylists)
      .catch((e: unknown) => {
        setError(e instanceof Error ? e.message : "load failed");
        setPlaylists([]);
      });
  }, [activeModeId]);

  async function toggleExpand(id: number) {
    if (expanded === id) {
      setExpanded(null);
      return;
    }
    setExpanded(id);
    if (tracks[id] !== undefined) return;
    setTracks((prev) => ({ ...prev, [id]: "loading" }));
    try {
      const rows = await playlistsApi.tracks(id);
      setTracks((prev) => ({ ...prev, [id]: rows }));
    } catch (e) {
      setTracks((prev) => ({
        ...prev,
        [id]: { error: e instanceof Error ? e.message : "load failed" },
      }));
    }
  }

  function play(playlistId: number) {
    wsClient.send({ type: "ambient_play_playlist", playlist_id: playlistId });
  }
  function playTrack(trackId: number) {
    wsClient.send({ type: "ambient_play_track", track_id: trackId });
  }
  function enqueueTrack(trackId: number) {
    wsClient.send({ type: "ambient_enqueue", track_id: trackId });
  }

  if (error !== null) return <p className="error small">{error}</p>;
  if (playlists.length === 0) {
    return (
      <p className="muted small">
        {activeModeId !== null
          ? `No playlists for mode "${activeModeId}" or global.`
          : "No playlists yet."}
      </p>
    );
  }

  return (
    <ul className="playlist-list">
      {playlists.map((p) => {
        const isOpen = expanded === p.id;
        const rows = tracks[p.id];
        return (
          <li key={p.id} className="playlist-list-item-wrap">
            <div className="playlist-list-item">
              <button
                type="button"
                className="playlist-disclosure"
                onClick={() => void toggleExpand(p.id)}
                title={isOpen ? "Collapse" : "Expand to view tracks"}
              >
                {isOpen ? "▾" : "▸"}
              </button>
              <div className="playlist-list-item-meta">
                <span className="playlist-name">{p.name}</span>
                <span className="muted small">
                  {p.category !== null ? p.category : ""}
                  {p.mode_id !== null ? ` · ${p.mode_id}` : " · global"}
                </span>
              </div>
              <button
                type="button"
                onClick={() => play(p.id)}
                title="Replace ambient lane with this playlist"
              >
                Play
              </button>
            </div>
            {isOpen ? (
              <div className="playlist-tracks">
                {rows === "loading" ? (
                  <p className="muted small">Loading tracks…</p>
                ) : rows === undefined ? null : "error" in rows ? (
                  <p className="error small">{rows.error}</p>
                ) : rows.length === 0 ? (
                  <p className="muted small">Empty playlist.</p>
                ) : (
                  <ol className="playlist-track-list">
                    {rows.map((r) => {
                      const isPlaying = activeTrackId === r.track_id;
                      const label =
                        r.track?.title ||
                        (r.track?.path ?? `Track ${r.track_id}`);
                      const subtitle = r.track
                        ? `${r.track.artist}${
                            r.track.album ? ` · ${r.track.album}` : ""
                          }`
                        : null;
                      return (
                        <li
                          key={`${r.position}-${r.track_id}`}
                          className={`playlist-track ${isPlaying ? "playing" : ""}`}
                        >
                          <span className="playlist-track-pos muted small">
                            {r.position + 1}
                          </span>
                          <div className="playlist-track-meta">
                            <span className="playlist-track-title">{label}</span>
                            {subtitle !== null ? (
                              <span className="muted small">{subtitle}</span>
                            ) : null}
                          </div>
                          <div className="playlist-track-actions">
                            <button onClick={() => playTrack(r.track_id)} title="Play">
                              ▶
                            </button>
                            <button
                              onClick={() => enqueueTrack(r.track_id)}
                              title="Queue"
                            >
                              ＋
                            </button>
                          </div>
                        </li>
                      );
                    })}
                  </ol>
                )}
              </div>
            ) : null}
          </li>
        );
      })}
    </ul>
  );
}
