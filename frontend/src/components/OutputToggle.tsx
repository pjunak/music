import { useEffect, useRef, useState } from "react";

import { devicesApi } from "@/core/api";
import { useAuthStore } from "@/core/auth";
import { playbackEngine } from "@/core/playbackEngine";
import {
  selectIsMyOutput,
  usePlayerStore,
} from "@/core/playerStore";
import { toast } from "@/core/toast";
import { defaultDeviceName, useUiStore } from "@/core/uiStore";
import { wsClient } from "@/core/ws";

import { VolumeIcon } from "./icons";

/** Compact "is this device an audio output?" toggle for the NowPlayingBar.
 *
 *  - Logged-in user → activate / deactivate this device via `set_active_outputs`.
 *    If this device isn't a designated output yet, the *first* enable also
 *    designates it (`PUT /api/devices/{clientId}` with `is_output: true`). That
 *    designation IS the manual operator action the output model requires — we
 *    just let the operator perform it from their own footer toggle instead of
 *    forcing a detour to Settings → Devices. It still never happens implicitly
 *    (it takes this click), and because activation is session-only, a later
 *    refresh shows the device designated-but-OFF rather than auto-playing.
 *  - Guest → flip the local-only `forceLocalPlayback` UI flag instead, since
 *    the server won't accept output-membership changes from guest sockets.
 *
 *  States we render:
 *    - Connecting: muted "Connecting…" pill, no action.
 *    - Authed + OFF: a clickable "Output OFF" pill (enables, self-designating
 *      on first use).
 *    - Authed + ON: "Output ON", arm-to-silence on the way back off.
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
  const me = usePlayerStore((s) =>
    s.state?.connected_devices.find((d) => d.device_id === s.myDeviceId) ?? null,
  );
  const authStatus = useAuthStore((s) => s.status);
  const isGuest = authStatus !== "authenticated";
  const forceLocal = useUiStore((s) => s.forceLocalPlayback);
  const clientId = useUiStore((s) => s.clientId);
  const deviceName = useUiStore((s) => s.deviceName);

  // Hooks declared up here (before any conditional returns) so React's
  // rules-of-hooks invariant holds across the connecting / guest / authed
  // branches below. The `armed` confirm-to-silence and `busy` states are
  // only meaningful in the authenticated branch but it costs nothing to
  // keep the hooks called in every render.
  const [armed, setArmed] = useState(false);
  // True while the self-designate PUT is in flight, so a double-click can't
  // fire two designations / activations.
  const [busy, setBusy] = useState(false);
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

  const designated = me?.is_output ?? false;

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
    // Derive the next list from the LIVE store, not the render-time closure:
    // `set_active_outputs` replaces the whole list, and the async self-designate
    // PUT (plus any concurrent change) can land between render and click — using
    // a stale snapshot here would clobber it. `includes` also dedupes the add.
    const player = usePlayerStore.getState();
    const current = player.state?.active_output_device_ids ?? [];
    const mine = current.includes(myDeviceId);
    const nextIds = mine
      ? current.filter((d) => d !== myDeviceId)
      : [...current, myDeviceId];

    // Optimistic: drive the engine directly off the new value so the
    // audio reacts on the click event, not after the WS roundtrip. The
    // subsequent state_changed broadcast re-applies the same state via
    // the store subscription — applyState is idempotent so the second
    // call is a no-op.
    playbackEngine.unlock();
    if (player.state !== null) {
      playbackEngine.applyState(
        { ...player.state, active_output_device_ids: nextIds },
        !mine,
      );
    }

    wsClient.send({ type: "set_active_outputs", device_ids: nextIds });
  }

  /** Enable this device as an output. If it isn't a designated output yet,
   *  designate it first (the explicit, manual operator action the model
   *  requires) — then activate. Designation persists; activation does not. */
  async function enableOutput() {
    if (myDeviceId === null || busy) return;
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
    if (myDeviceId === null) return;
    if (!isMyOutput) {
      // Going ON — no confirmation, the operator wants audio. Self-designates
      // this device first if it isn't a designated output yet.
      void enableOutput();
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
