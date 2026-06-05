import { useEffect, useRef, useState } from "react";

import { devicesApi } from "@/core/api";
import { useAuthStore } from "@/core/auth";
import { playbackEngine } from "@/core/playbackEngine";
import { selectIsMyOutput, usePlayerStore } from "@/core/playerStore";
import { toast } from "@/core/toast";
import { defaultDeviceName, useUiStore } from "@/core/uiStore";
import { wsClient } from "@/core/ws";

import { VolumeIcon } from "./icons";

/** "Is this device an audio output?" pill for the NowPlayingBar.
 *
 *  Three distinct surfaces, dispatched by connection + auth state — each is its
 *  own component so it declares only the hooks it actually needs:
 *    - no snapshot yet → a muted, non-interactive "Connecting…" pill;
 *    - guest          → flips the local-only `forceLocalPlayback` flag (the
 *                       server rejects output changes from guest sockets);
 *    - authed         → activates/deactivates via `set_active_outputs`,
 *                       self-designating this device on first enable.
 *
 *  The interactive surfaces optimistically call `playbackEngine.applyState` on
 *  click so audio reacts on the gesture, not after the WS round-trip; the
 *  echoing `state_changed` re-applies idempotently. */
export function OutputToggle() {
  const myDeviceId = usePlayerStore((s) => s.myDeviceId);
  const isGuest = useAuthStore((s) => s.status) !== "authenticated";

  if (myDeviceId === null) return <ConnectingPill />;
  return isGuest ? (
    <GuestOutputToggle />
  ) : (
    <AuthedOutputToggle deviceId={myDeviceId} />
  );
}

function ConnectingPill() {
  return (
    <span className="output-toggle output-toggle-idle">
      <VolumeIcon className="output-toggle-icon" />
      <span className="output-toggle-label muted">Connecting…</span>
    </span>
  );
}

/** Guest fallback: the server rejects output-membership changes from guest
 *  sockets, so flip the local-only `forceLocalPlayback` flag to hear audio on
 *  this tab. */
function GuestOutputToggle() {
  const active = useUiStore((s) => s.forceLocalPlayback);
  const setForceLocal = useUiStore((s) => s.setForceLocalPlayback);

  function toggle() {
    const next = !active;
    setForceLocal(next);
    // Optimistic: tell the engine right away so audio reacts before the
    // UI-store subscription roundtrip.
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
          : "Audio output is OFF for this device. Click to play locally (sign in to share with the operator)."
      }
      aria-label={active ? "Stop playing here" : "Play on this device"}
      aria-pressed={active}
    >
      <VolumeIcon className="output-toggle-icon" />
      <span className="output-toggle-label">
        {active ? "Output ON · local" : "Output OFF"}
      </span>
    </button>
  );
}

/** Authed control: activate/deactivate this device as a server-side output.
 *
 *  - OFF → ON: one click. Self-designates this device (`PUT is_output:true`) if
 *    it isn't a designated output yet — the explicit manual designation the
 *    output model requires, just performed from the operator's own footer
 *    instead of a Settings → Devices detour. Designation persists; activation is
 *    session-only, so a later refresh comes back designated-but-OFF rather than
 *    auto-playing.
 *  - ON → OFF: arm-to-confirm. Silencing mid-session is the expensive mistake,
 *    so the first click only arms ("Click again to stop"); a second within 2s
 *    actually silences. Works identically for mouse and keyboard. */
function AuthedOutputToggle({ deviceId }: { deviceId: string }) {
  const isMyOutput = usePlayerStore(selectIsMyOutput);
  // A boolean selector (stable primitive) — not the `me` object, whose ref
  // churns on every broadcast.
  const designated = usePlayerStore(
    (s) =>
      s.state?.connected_devices.find((d) => d.device_id === deviceId)
        ?.is_output ?? false,
  );
  const clientId = useUiStore((s) => s.clientId);
  const deviceName = useUiStore((s) => s.deviceName);

  const [armed, setArmed] = useState(false);
  // True while the self-designate PUT is in flight, so a double-click can't
  // fire two designations / activations.
  const [busy, setBusy] = useState(false);
  const armTimeoutRef = useRef<number | null>(null);
  useEffect(
    () => () => {
      if (armTimeoutRef.current !== null) {
        window.clearTimeout(armTimeoutRef.current);
      }
    },
    [],
  );

  /** Activate or deactivate this device. Derives the next list from the LIVE
   *  store (not a render-time closure): `set_active_outputs` replaces the whole
   *  list, and the async designate PUT (or a concurrent change) can land between
   *  render and click — a stale snapshot would clobber it. `includes` dedupes. */
  function commitToggle() {
    const player = usePlayerStore.getState();
    const current = player.state?.active_output_device_ids ?? [];
    const mine = current.includes(deviceId);
    const nextIds = mine
      ? current.filter((d) => d !== deviceId)
      : [...current, deviceId];

    playbackEngine.unlock();
    if (player.state !== null) {
      playbackEngine.applyState(
        { ...player.state, active_output_device_ids: nextIds },
        !mine,
      );
    }
    wsClient.send({ type: "set_active_outputs", device_ids: nextIds });
  }

  async function enableOutput() {
    if (busy) return;
    if (!designated) {
      setBusy(true);
      try {
        await devicesApi.save(clientId, {
          name: deviceName ?? defaultDeviceName(),
          is_output: true,
        });
        // First-enable only: surface the persistent side-effect (this device is
        // now in the registry) so designating isn't a silent, invisible change.
        toast.success(
          "This device is now an audio output",
          "Manage or remove it in Settings → Devices.",
        );
      } catch (err) {
        toast.error(
          "Couldn't set this device as an output",
          err instanceof Error ? err.message : undefined,
        );
        return;
      } finally {
        setBusy(false);
      }
    }
    commitToggle();
  }

  function clearArm() {
    if (armTimeoutRef.current !== null) {
      window.clearTimeout(armTimeoutRef.current);
      armTimeoutRef.current = null;
    }
    setArmed(false);
  }

  function onClick() {
    if (!isMyOutput) {
      // Going ON — no confirmation, the operator wants audio. Self-designates
      // first if needed.
      void enableOutput();
      return;
    }
    if (armed) {
      // Second click of the arm-then-confirm sequence.
      clearArm();
      commitToggle();
      return;
    }
    // First click on an active output — arm it; a second within 2s commits,
    // otherwise the arm self-resets.
    setArmed(true);
    armTimeoutRef.current = window.setTimeout(() => {
      armTimeoutRef.current = null;
      setArmed(false);
    }, 2000);
  }

  const stateClass = isMyOutput
    ? armed
      ? "output-toggle-on output-toggle-armed"
      : "output-toggle-on"
    : "output-toggle-off";
  const label = isMyOutput
    ? armed
      ? "Click again to stop"
      : "Output ON"
    : busy
      ? "Enabling…"
      : "Output OFF";
  const tooltip = isMyOutput
    ? armed
      ? "Click again within 2s to silence this device."
      : "Audio output is ON. Click once to arm; click again to silence."
    : designated
      ? "Audio output is OFF for this device. Click to enable."
      : "Click to make this device an audio output and play here.";

  return (
    <button
      type="button"
      className={`output-toggle ${stateClass}`}
      onClick={onClick}
      onBlur={clearArm}
      disabled={busy}
      title={tooltip}
      aria-label={isMyOutput ? "Stop playing here" : "Play on this device"}
      aria-pressed={isMyOutput}
    >
      <VolumeIcon className="output-toggle-icon" />
      <span className="output-toggle-label">{label}</span>
    </button>
  );
}
