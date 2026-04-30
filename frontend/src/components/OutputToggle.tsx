import { useAuthStore } from "@/core/auth";
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
 *    - User without `audio_output` capability: muted hint, opens Settings
 *      isn't reachable from here — so we just say so and leave it.
 *    - Otherwise: a clickable pill that toggles between
 *      "Playing here" (active) and "Play here" (idle). */

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
    return (
      <button
        type="button"
        className={`output-toggle ${active ? "output-toggle-on" : "output-toggle-off"}`}
        onClick={() => setForceLocal(!active)}
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

  if (me === null || !me.capabilities.includes("audio_output")) {
    return (
      <span
        className="output-toggle output-toggle-idle"
        title="Enable Audio output in Settings → This device"
      >
        <VolumeIcon className="output-toggle-icon" />
        <span className="output-toggle-label muted">Not an output</span>
      </span>
    );
  }

  function toggle() {
    if (myDeviceId === null) return;
    if (isMyOutput) {
      if (activeIds === undefined) return;
      wsClient.send({
        type: "set_active_outputs",
        device_ids: activeIds.filter((d) => d !== myDeviceId),
      });
    } else {
      const next = activeIds === undefined ? [myDeviceId] : [...activeIds, myDeviceId];
      wsClient.send({ type: "set_active_outputs", device_ids: next });
    }
  }

  return (
    <button
      type="button"
      className={`output-toggle ${isMyOutput ? "output-toggle-on" : "output-toggle-off"}`}
      onClick={toggle}
      title={
        isMyOutput
          ? "Audio output is ON for this device. Click to silence."
          : "Audio output is OFF for this device. Click to enable."
      }
      aria-label={isMyOutput ? "Stop playing here" : "Play on this device"}
      aria-pressed={isMyOutput}
    >
      <VolumeIcon className="output-toggle-icon" />
      <span className="output-toggle-label">
        {isMyOutput ? "Output ON" : "Output OFF"}
      </span>
    </button>
  );
}
