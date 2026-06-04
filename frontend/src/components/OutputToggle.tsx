import { useEffect, useRef, useState } from "react";

import { useAuthStore } from "@/core/auth";
import { playbackEngine } from "@/core/playbackEngine";
import {
  selectIsMyOutput,
  usePlayerStore,
} from "@/core/playerStore";
import { useUiStore } from "@/core/uiStore";
import { wsClient } from "@/core/ws";

import { VolumeIcon } from "./icons";

/** Compact "is this device an audio output?" toggle for the NowPlayingBar.
 *
 *  Same logic as the larger badge that used to live on the Player view:
 *  - Logged-in user → claim / release via `set_active_outputs`.
 *  - Guest → flip the local-only `forceLocalPlayback` UI flag instead, since
 *    the server won't accept output-membership changes from guest sockets.
 *
 *  States we render:
 *    - Connecting: muted "Connecting…" pill, no action.
 *    - Device not designated as an output: muted "Not an output" hint — the
 *      operator marks it in Settings → Devices first (output is fully manual).
 *    - Otherwise: a clickable pill that toggles between
 *      "Playing here" (active) and "Play here" (idle).
 *
 *  Optimistic update: in addition to sending the WS / flipping the UI flag,
 *  we tell `playbackEngine.applyState` directly with the optimistic new
 *  isMyOutput value. Without this the engine only reacts when the WS
 *  broadcast comes back through the store subscription, and the operator
 *  perceives a "click does nothing until refresh" lag. The subsequent
 *  state_changed broadcast re-applies idempotently. */

export function OutputToggle() {
  const isMyOutput = usePlayerStore(selectIsMyOutput);
  const myDeviceId = usePlayerStore((s) => s.myDeviceId);
  const activeIds = usePlayerStore((s) => s.state?.active_output_device_ids);
  const me = usePlayerStore((s) =>
    s.state?.connected_devices.find((d) => d.device_id === s.myDeviceId) ?? null,
  );
  const authStatus = useAuthStore((s) => s.status);
  const isGuest = authStatus !== "authenticated";
  const forceLocal = useUiStore((s) => s.forceLocalPlayback);

  // Hooks declared up here (before any conditional returns) so React's
  // rules-of-hooks invariant holds across the connecting / guest /
  // no-capability branches below. The `armed` confirm-to-silence state
  // is only meaningful in the authenticated branch but it costs nothing
  // to keep the hook called in every render.
  const [armed, setArmed] = useState(false);
  const armTimeoutRef = useRef<number | null>(null);
  useEffect(() => {
    return () => {
      if (armTimeoutRef.current !== null) {
        window.clearTimeout(armTimeoutRef.current);
      }
    };
  }, []);
  const setForceLocal = useUiStore((s) => s.setForceLocalPlayback);

  if (myDeviceId === null) {
    return (
      <span className="output-toggle output-toggle-idle">
        <VolumeIcon className="output-toggle-icon" />
        <span className="output-toggle-label muted">Connecting…</span>
      </span>
    );
  }

  if (isGuest) {
    const active = forceLocal;
    function toggleGuest() {
      const next = !active;
      setForceLocal(next);
      // Optimistic: tell the engine right away so audio reacts before
      // the UI-store subscription roundtrip.
      playbackEngine.unlock();
      const player = usePlayerStore.getState();
      if (player.state !== null) {
        playbackEngine.applyState(player.state, next);
      }
    }
    return (
      <button
        type="button"
        className={`output-toggle ${active ? "output-toggle-on" : "output-toggle-off"}`}
        onClick={toggleGuest}
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

  if (me === null || !me.is_output) {
    return (
      <span
        className="output-toggle output-toggle-idle"
        title="Mark this device as an audio output in Settings → Devices to play here"
      >
        <VolumeIcon className="output-toggle-icon" />
        <span className="output-toggle-label muted">Not an output</span>
      </span>
    );
  }

  // Confirm-to-silence guard. Stopping audio mid-session is the
  // expensive mistake; turning it on is harmless. So:
  //   - OFF → ON: single click, no friction.
  //   - ON → OFF: first click arms the button (label flips to "Click
  //     again to stop", warn-coloured ring), second click within 2s
  //     actually silences. Out-of-window clicks reset the arm.
  //
  // Works the same for keyboard (Enter twice on the focused button) and
  // mouse, no `onPointerDown`/`onPointerUp` complexity needed.

  function commitToggle() {
    if (myDeviceId === null) return;
    const nextIds = isMyOutput
      ? (activeIds ?? []).filter((d) => d !== myDeviceId)
      : activeIds === undefined
        ? [myDeviceId]
        : [...activeIds, myDeviceId];
    const willBeMyOutput = nextIds.includes(myDeviceId);

    // Optimistic: drive the engine directly off the new value so the
    // audio reacts on the click event, not after the WS roundtrip. The
    // subsequent state_changed broadcast re-applies the same state via
    // the store subscription — applyState is idempotent so the second
    // call is a no-op.
    playbackEngine.unlock();
    const player = usePlayerStore.getState();
    if (player.state !== null) {
      playbackEngine.applyState(
        { ...player.state, active_output_device_ids: nextIds },
        willBeMyOutput,
      );
    }

    wsClient.send({ type: "set_active_outputs", device_ids: nextIds });
  }

  function clearArm() {
    if (armTimeoutRef.current !== null) {
      window.clearTimeout(armTimeoutRef.current);
      armTimeoutRef.current = null;
    }
    setArmed(false);
  }

  function onClick() {
    if (myDeviceId === null) return;
    if (!isMyOutput) {
      // Going ON — no confirmation, the operator wants audio.
      commitToggle();
      return;
    }
    if (armed) {
      // Second click of the arm-then-confirm sequence.
      clearArm();
      commitToggle();
      return;
    }
    // First click on an active output — arm it. A second click within
    // 2s commits; otherwise the arm self-resets.
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
    : "Output OFF";
  const tooltip = isMyOutput
    ? armed
      ? "Click again within 2s to silence this device."
      : "Audio output is ON. Click once to arm; click again to silence."
    : "Audio output is OFF for this device. Click to enable.";

  return (
    <button
      type="button"
      className={`output-toggle ${stateClass}`}
      onClick={onClick}
      onBlur={clearArm}
      title={tooltip}
      aria-label={isMyOutput ? "Stop playing here" : "Play on this device"}
      aria-pressed={isMyOutput}
    >
      <VolumeIcon className="output-toggle-icon" />
      <span className="output-toggle-label">{label}</span>
    </button>
  );
}
