import { useEffect, useRef, useState } from "react";
import { Link } from "react-router-dom";

import { useAuthStore } from "@/core/auth";
import { shortDeviceLabel } from "@/core/deviceLabel";
import { playbackEngine } from "@/core/playbackEngine";
import {
  selectIsMyOutput,
  usePlayerArray,
  usePlayerStore,
} from "@/core/playerStore";
import { useUiStore } from "@/core/uiStore";
import { wsClient } from "@/core/ws";

import { VolumeControl } from "./VolumeControl";
import { VolumeIcon } from "./icons";

// Stable empty default for the device_volumes selector (see the
// local/stable-store-selector lint rule).
const EMPTY_VOLUMES: Record<string, number> = {};

/** Footer "Speakers" control — the single place to pick which devices output
 *  audio and balance their volumes (the Sonos model). Replaces the old
 *  single-pill OutputToggle *and* the Console's separate Outputs bar.
 *
 *   - Connecting (no snapshot yet) → a muted pill.
 *   - Guest → a local-only on/off (the server rejects output changes from guest
 *     sockets) via `forceLocalPlayback`; no multi-device popover.
 *   - Authed → a pill showing how many speakers are on (green when THIS device
 *     is one) that opens a popover listing EVERY connected device with an
 *     on/off toggle + per-device volume. Ticking a device on makes it a live
 *     output for this session — no pre-designation needed. A "default" badge
 *     marks devices saved as output-by-default (Settings → Devices), which
 *     auto-activate when they connect. */
export function SpeakersControl() {
  const myDeviceId = usePlayerStore((s) => s.myDeviceId);
  const isGuest = useAuthStore((s) => s.status) !== "authenticated";

  if (myDeviceId === null) {
    return (
      <span className="output-toggle output-toggle-idle">
        <VolumeIcon className="output-toggle-icon" />
        <span className="output-toggle-label muted">Connecting…</span>
      </span>
    );
  }
  return isGuest ? <GuestSpeaker /> : <AuthedSpeakers deviceId={myDeviceId} />;
}

/** Guest fallback: flip the local-only `forceLocalPlayback` flag (the server
 *  won't accept output-membership changes from guest sockets). */
function GuestSpeaker() {
  const active = useUiStore((s) => s.forceLocalPlayback);
  const setForceLocal = useUiStore((s) => s.setForceLocalPlayback);
  function toggle() {
    const next = !active;
    setForceLocal(next);
    playbackEngine.unlock();
    const player = usePlayerStore.getState();
    if (player.state !== null) playbackEngine.applyState(player.state, next);
  }
  return (
    <button
      type="button"
      className={`output-toggle ${active ? "output-toggle-on" : "output-toggle-off"}`}
      onClick={toggle}
      title={
        active
          ? "Audio output is ON for this device (local-only). Click to silence."
          : "Audio output is OFF. Click to play locally (sign in to share with the operator)."
      }
      aria-pressed={active}
    >
      <VolumeIcon className="output-toggle-icon" />
      <span className="output-toggle-label">
        {active ? "Output ON · local" : "Output OFF"}
      </span>
    </button>
  );
}

function AuthedSpeakers({ deviceId }: { deviceId: string }) {
  const [open, setOpen] = useState(false);
  const rootRef = useRef<HTMLDivElement | null>(null);

  const isMyOutput = usePlayerStore(selectIsMyOutput);
  const devices = usePlayerArray((s) => s.state?.connected_devices);
  const activeIds = usePlayerArray((s) => s.state?.active_output_device_ids);
  const deviceVolumes =
    usePlayerStore((s) => s.state?.device_volumes) ?? EMPTY_VOLUMES;
  const deviceName = useUiStore((s) => s.deviceName);

  useEffect(() => {
    if (!open) return;
    function onDown(e: PointerEvent) {
      if (rootRef.current && !rootRef.current.contains(e.target as Node)) {
        setOpen(false);
      }
    }
    function onKey(e: KeyboardEvent) {
      if (e.key === "Escape") setOpen(false);
    }
    window.addEventListener("pointerdown", onDown);
    window.addEventListener("keydown", onKey);
    return () => {
      window.removeEventListener("pointerdown", onDown);
      window.removeEventListener("keydown", onKey);
    };
  }, [open]);

  const thisDevice = devices.find((d) => d.device_id === deviceId) ?? null;
  const others = devices.filter((d) => d.device_id !== deviceId);

  /** Turn a device on/off as a live output. No designation needed — any
   *  connected device can be activated. Reads the live active list to avoid
   *  clobbering a concurrent change. */
  function setOn(id: string, on: boolean) {
    const player = usePlayerStore.getState();
    const current = player.state?.active_output_device_ids ?? [];
    const next = on
      ? current.includes(id)
        ? current
        : [...current, id]
      : current.filter((d) => d !== id);
    // Optimistic for THIS device so its audio reacts on the click.
    if (id === deviceId) {
      playbackEngine.unlock();
      // Turning our own output off: flush a final position report while we're
      // still an active output (queued before set_active_outputs on the same
      // ordered socket, so the server accepts it) — otherwise its position_ms
      // stays frozen at the last 1s report and a quick off→on resumes from that
      // stale second (a small backward jump / replaying the same second).
      if (!next.includes(deviceId)) {
        playbackEngine.flushPositionReport();
      }
      if (player.state !== null) {
        playbackEngine.applyState(
          { ...player.state, active_output_device_ids: next },
          next.includes(deviceId),
        );
      }
    }
    wsClient.send({ type: "set_active_outputs", device_ids: next });
  }

  function setVol(id: string, v: number) {
    wsClient.send({ type: "set_device_volume", device_id: id, volume: v });
  }

  return (
    <div className="speakers-control" ref={rootRef}>
      <button
        type="button"
        className={`output-toggle ${isMyOutput ? "output-toggle-on" : "output-toggle-off"}`}
        onClick={() => setOpen((o) => !o)}
        aria-haspopup="dialog"
        aria-expanded={open}
        title="Choose which speakers play and balance their volumes"
      >
        <VolumeIcon className="output-toggle-icon" />
        <span className="output-toggle-label">
          {activeIds.length === 0 ? "Speakers" : `Speakers · ${activeIds.length}`}
        </span>
      </button>
      {open ? (
        <div className="speakers-popover" role="dialog" aria-label="Speakers">
          <div className="speakers-popover-head">
            <span>Speakers</span>
            <Link
              to="/settings"
              className="btn-link-external"
              onClick={() => setOpen(false)}
            >
              ⚙ Manage
            </Link>
          </div>
          <SpeakerRow
            name={`${shortDeviceLabel(thisDevice?.name ?? deviceName ?? "This device")} (this)`}
            on={activeIds.includes(deviceId)}
            isDefault={thisDevice?.is_output ?? false}
            volume={deviceVolumes[deviceId] ?? 1}
            onToggle={(on) => setOn(deviceId, on)}
            onVolume={(v) => setVol(deviceId, v)}
          />
          {others.map((d) => (
            <SpeakerRow
              key={d.device_id}
              name={shortDeviceLabel(d.name)}
              on={activeIds.includes(d.device_id)}
              isDefault={d.is_output}
              volume={deviceVolumes[d.device_id] ?? 1}
              onToggle={(on) => setOn(d.device_id, on)}
              onVolume={(v) => setVol(d.device_id, v)}
            />
          ))}
        </div>
      ) : null}
    </div>
  );
}

function SpeakerRow({
  name,
  on,
  isDefault,
  volume,
  onToggle,
  onVolume,
}: {
  name: string;
  on: boolean;
  isDefault?: boolean;
  volume: number;
  onToggle: (on: boolean) => void;
  onVolume: (v: number) => void;
}) {
  return (
    <div className={`speaker-row${on ? " on" : ""}`}>
      <label className="speaker-row-toggle">
        <input
          type="checkbox"
          checked={on}
          onChange={(e) => onToggle(e.target.checked)}
        />
        <span className="speaker-row-name">{name}</span>
        {isDefault ? (
          <span
            className="speaker-row-default"
            title="Output on by default — auto-activates when this device connects (Settings → Devices)"
          >
            default
          </span>
        ) : null}
      </label>
      <VolumeControl
        value={volume}
        onChange={onVolume}
        label={`${name} volume`}
        showIcon={false}
        className="speaker-row-vol"
      />
    </div>
  );
}
