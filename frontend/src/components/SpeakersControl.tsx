import { useEffect, useRef, useState } from "react";
import { Link } from "react-router-dom";

import { devicesApi } from "@/core/api";
import { useAuthStore } from "@/core/auth";
import { playbackEngine } from "@/core/playbackEngine";
import {
  selectIsMyOutput,
  usePlayerArray,
  usePlayerStore,
} from "@/core/playerStore";
import { toast } from "@/core/toast";
import { defaultDeviceName, useUiStore } from "@/core/uiStore";
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
 *     is one) that opens a popover listing each designated output with an on/off
 *     toggle + a per-device volume slider. */
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
  const [busy, setBusy] = useState(false);
  const rootRef = useRef<HTMLDivElement | null>(null);

  const isMyOutput = usePlayerStore(selectIsMyOutput);
  const devices = usePlayerArray((s) => s.state?.connected_devices);
  const activeIds = usePlayerArray((s) => s.state?.active_output_device_ids);
  const deviceVolumes =
    usePlayerStore((s) => s.state?.device_volumes) ?? EMPTY_VOLUMES;
  const clientId = useUiStore((s) => s.clientId);
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
  const otherOutputs = devices.filter(
    (d) => d.is_output && d.device_id !== deviceId,
  );

  /** Turn a device on/off. `selfDesignate` (this device only) does the explicit
   *  PUT is_output=true on first enable. Reads the live active list to avoid
   *  clobbering a concurrent change. */
  async function setOn(id: string, on: boolean, selfDesignate: boolean) {
    if (busy) return;
    if (selfDesignate && on) {
      setBusy(true);
      try {
        await devicesApi.save(clientId, {
          name: deviceName ?? defaultDeviceName(),
          is_output: true,
        });
        toast.success(
          "This device is now an audio output",
          "Manage or remove it in Settings → Devices.",
        );
      } catch (e) {
        toast.error(
          "Couldn't set this device as an output",
          e instanceof Error ? e.message : undefined,
        );
        return;
      } finally {
        setBusy(false);
      }
    }
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
            name={`${thisDevice?.name ?? deviceName ?? "This device"} (this)`}
            on={activeIds.includes(deviceId)}
            volume={deviceVolumes[deviceId] ?? 1}
            busy={busy}
            onToggle={(on) => void setOn(deviceId, on, !(thisDevice?.is_output ?? false))}
            onVolume={(v) => setVol(deviceId, v)}
          />
          {otherOutputs.map((d) => (
            <SpeakerRow
              key={d.device_id}
              name={d.name}
              on={activeIds.includes(d.device_id)}
              volume={deviceVolumes[d.device_id] ?? 1}
              onToggle={(on) => void setOn(d.device_id, on, false)}
              onVolume={(v) => setVol(d.device_id, v)}
            />
          ))}
          {otherOutputs.length === 0 ? (
            <p className="muted small speakers-empty">
              No other speakers yet. Open this app on a TV / speaker tab, then
              mark it as an output in Settings → Devices.
            </p>
          ) : null}
        </div>
      ) : null}
    </div>
  );
}

function SpeakerRow({
  name,
  on,
  volume,
  busy,
  onToggle,
  onVolume,
}: {
  name: string;
  on: boolean;
  volume: number;
  busy?: boolean;
  onToggle: (on: boolean) => void;
  onVolume: (v: number) => void;
}) {
  return (
    <div className={`speaker-row${on ? " on" : ""}`}>
      <label className="speaker-row-toggle">
        <input
          type="checkbox"
          checked={on}
          disabled={busy}
          onChange={(e) => onToggle(e.target.checked)}
        />
        <span className="speaker-row-name">{name}</span>
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
