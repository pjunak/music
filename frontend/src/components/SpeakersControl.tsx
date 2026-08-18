import { useEffect, useRef, useState } from "react";
import { Link } from "react-router-dom";

import { useAuthStore } from "@/core/auth";
import { deviceDisplayName } from "@/core/deviceVisual";
import { playbackEngine } from "@/core/playbackEngine";
import { usePlayerArray, usePlayerStore } from "@/core/playerStore";
import { useUiStore } from "@/core/uiStore";
import { wsClient } from "@/core/ws";

import { DeviceIcon } from "./DeviceIcon";
import { VolumeControl } from "./VolumeControl";
import { VolumeIcon } from "./icons";

// Stable empty default for the device_volumes selector (see the
// local/stable-store-selector lint rule).
const EMPTY_VOLUMES: Record<string, number> = {};

/** Footer "Speakers" control — pick the current output and control its volume.
 *  Single-output use stays directly accessible in the footer; multi-output is
 *  an explicit option in the popover.
 *
 *   - Connecting (no snapshot yet) → a muted pill.
 *   - Guest → a local-only on/off (the server rejects output changes from guest
 *     sockets) via `forceLocalPlayback`; no multi-device popover.
 *   - Authed → a pill showing how many speakers are on. With exactly one active
 *     output, its volume is exposed beside the pill. The popover lists every
 *     connected device; selecting one replaces the current output unless
 *     "Multiple" is enabled. A "default" badge marks devices saved as
 *     output-by-default (Settings → Devices), which auto-activate on connect. */
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

  const devices = usePlayerArray((s) => s.state?.connected_devices);
  const activeIds = usePlayerArray((s) => s.state?.active_output_device_ids);
  const [multiple, setMultiple] = useState(() => activeIds.length > 1);
  const deviceVolumes =
    usePlayerStore((s) => s.state?.device_volumes) ?? EMPTY_VOLUMES;
  const defaultDeviceVolume = usePlayerStore((s) => s.state?.default_device_volume);
  const legacyMasterVolume = usePlayerStore((s) => s.state?.volume ?? 1);
  const connected = usePlayerStore((s) => s.wsStatus === "connected");
  const deviceName = useUiStore((s) => s.deviceName);

  // A second controller or an output-by-default connection can create a
  // multi-output session outside this popover. Reflect that canonical state
  // instead of presenting it as single-output mode.
  useEffect(() => {
    if (activeIds.length > 1) setMultiple(true);
  }, [activeIds]);

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
  const singleOutputId = activeIds.length === 1 ? activeIds[0] : null;
  const singleOutput =
    singleOutputId === null
      ? null
      : devices.find((device) => device.device_id === singleOutputId) ?? null;

  function volumeFor(id: string): number {
    const stored = deviceVolumes[id];
    return defaultDeviceVolume === undefined
      ? legacyMasterVolume * (stored ?? 1)
      : (stored ?? defaultDeviceVolume);
  }

  /** Commit output membership and immediately reconcile this browser's audio
   *  when the change also adds or removes this device. */
  function setActiveOutputs(next: string[]) {
    if (!connected) return;
    const player = usePlayerStore.getState();
    const current = player.state?.active_output_device_ids ?? [];
    const wasThisDevice = current.includes(deviceId);
    const isThisDevice = next.includes(deviceId);

    // Optimistic for this device so its audio reacts on the click, including
    // when choosing a different single output replaces this browser.
    if (wasThisDevice !== isThisDevice) {
      playbackEngine.unlock();
      // Turning our own output off: flush a final position report while we're
      // still an active output (queued before set_active_outputs on the same
      // ordered socket, so the server accepts it) — otherwise its position_ms
      // stays frozen at the last 1s report and a quick off→on resumes from that
      // stale second (a small backward jump / replaying the same second).
      if (!isThisDevice) {
        playbackEngine.flushPositionReport();
      }
      if (player.state !== null) {
        playbackEngine.applyState(
          { ...player.state, active_output_device_ids: next },
          isThisDevice,
        );
      }
    }
    wsClient.send({ type: "set_active_outputs", device_ids: next });
  }

  /** Turn a device on/off as a live output. In the default single-output mode,
   *  turning one device on replaces the current output. */
  function setOn(id: string, on: boolean) {
    const current = usePlayerStore.getState().state?.active_output_device_ids ?? [];
    const next = on
      ? multiple
        ? current.includes(id)
          ? current
          : [...current, id]
        : [id]
      : current.filter((activeId) => activeId !== id);
    setActiveOutputs(next);
  }

  function setMultipleOutputs(enabled: boolean) {
    setMultiple(enabled);
    if (enabled) return;

    const current = usePlayerStore.getState().state?.active_output_device_ids ?? [];
    if (current.length > 1) setActiveOutputs([current[0]]);
  }

  function setVol(id: string, v: number) {
    if (!connected) return;
    if (
      defaultDeviceVolume === undefined &&
      v > legacyMasterVolume
    ) {
      const knownIds = new Set([
        id,
        ...Object.keys(deviceVolumes),
        ...devices.map((device) => device.device_id),
      ]);
      const previousLevels = new Map(
        [...knownIds].map((deviceId) => [deviceId, volumeFor(deviceId)]),
      );
      wsClient.send({ type: "set_volume", volume: v });
      for (const deviceId of knownIds) {
        wsClient.send({
          type: "set_device_volume",
          device_id: deviceId,
          volume:
            deviceId === id
              ? 1
              : Math.min(1, (previousLevels.get(deviceId) ?? 0) / v),
        });
      }
      return;
    }
    const wireVolume =
      defaultDeviceVolume === undefined
        ? legacyMasterVolume > 0
          ? Math.min(1, v / legacyMasterVolume)
          : v === 0
            ? 0
            : 1
        : v;
    wsClient.send({ type: "set_device_volume", device_id: id, volume: wireVolume });
  }

  return (
    <div className="speakers-control" ref={rootRef}>
      {singleOutputId !== null ? (
        <VolumeControl
          value={volumeFor(singleOutputId)}
          onChange={(value) => setVol(singleOutputId, value)}
          label={`${deviceDisplayName(singleOutput?.name ?? singleOutputId)} volume in player bar`}
          showIcon={false}
          className="single-speaker-volume"
          readOnly={!connected}
          readOnlyTitle="Not connected"
        />
      ) : null}
      <button
        type="button"
        className={`output-toggle ${activeIds.length > 0 ? "output-toggle-on" : "output-toggle-off"}`}
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
            <label
              className="speakers-multiple-toggle"
              title="Allow more than one speaker to play at the same time"
            >
              <input
                type="checkbox"
                checked={multiple}
                disabled={!connected}
                onChange={(event) => setMultipleOutputs(event.target.checked)}
              />
              <span>Multiple</span>
            </label>
            <Link
              to="/settings"
              className="btn-link-external"
              onClick={() => setOpen(false)}
            >
              ⚙ Manage
            </Link>
          </div>
          <SpeakerRow
            rawName={thisDevice?.name ?? deviceName ?? "This device"}
            isThis
            on={activeIds.includes(deviceId)}
            isDefault={thisDevice?.is_output ?? false}
            volume={volumeFor(deviceId)}
            disabled={!connected}
            onToggle={(on) => setOn(deviceId, on)}
            onVolume={(v) => setVol(deviceId, v)}
          />
          {others.map((d) => (
            <SpeakerRow
              key={d.device_id}
              rawName={d.name}
              on={activeIds.includes(d.device_id)}
              isDefault={d.is_output}
              volume={volumeFor(d.device_id)}
              disabled={!connected}
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
  rawName,
  isThis = false,
  on,
  isDefault,
  volume,
  disabled,
  onToggle,
  onVolume,
}: {
  /** Source device name — drives both the icon and the (trimmed) label. */
  rawName: string;
  /** The operator's own device — gets a subtle frame instead of "(this)" text. */
  isThis?: boolean;
  on: boolean;
  isDefault?: boolean;
  volume: number;
  disabled: boolean;
  onToggle: (on: boolean) => void;
  onVolume: (v: number) => void;
}) {
  const name = deviceDisplayName(rawName);
  return (
    <div
      className={`speaker-row${on ? " on" : ""}${isThis ? " is-this" : ""}`}
      title={isThis ? "This device" : undefined}
    >
      <DeviceIcon name={rawName} className="speaker-row-icon" />
      <span className="speaker-row-name-wrap">
        <span className="speaker-row-name">{name}</span>
        {isDefault ? (
          <span
            className="speaker-row-default"
            title="Output on by default — auto-activates when this device connects (Settings → Devices)"
          >
            default
          </span>
        ) : null}
        {isThis ? <span className="sr-only">This device</span> : null}
      </span>
      <VolumeControl
        value={volume}
        onChange={onVolume}
        label={`${name} volume`}
        showIcon={false}
        className="speaker-row-vol"
        readOnly={disabled}
        readOnlyTitle="Not connected"
      />
      {/* Checkbox on the right edge — closest to the cursor when the popover
          opens above the footer pill. */}
      <input
        type="checkbox"
        className="speaker-row-check"
        checked={on}
        disabled={disabled}
        aria-label={`${name} output`}
        onChange={(e) => onToggle(e.target.checked)}
      />
    </div>
  );
}
