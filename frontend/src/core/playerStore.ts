import { create } from "zustand";

import type { PlayerState, WsMessage } from "@/core/types";
import type { WsStatus } from "@/core/ws";

interface PlayerStore {
  state: PlayerState | null;
  myDeviceId: string | null;
  wsStatus: WsStatus;
  /** Wall-clock timestamp (ms since epoch) when the latest state was received.
   *  Used by the player UI to dead-reckon the current playback position. */
  stateReceivedAt: number;

  applyMessage: (msg: WsMessage) => void;
  setStatus: (s: WsStatus) => void;
  reset: () => void;
}

export const usePlayerStore = create<PlayerStore>((set) => ({
  state: null,
  myDeviceId: null,
  wsStatus: "disconnected",
  stateReceivedAt: 0,

  applyMessage: (msg) => {
    if (msg.type === "state_snapshot") {
      set({
        state: msg.state,
        myDeviceId: msg.your_device_id,
        stateReceivedAt: Date.now(),
      });
    } else if (msg.type === "state_changed") {
      set({ state: msg.state, stateReceivedAt: Date.now() });
    }
    // sfx_fired / scene_activated / scene_deactivated / error handled by the
    // audio engine and toast layer respectively (not here — we only track
    // PlayerState in this store).
  },

  setStatus: (s) => {
    set({ wsStatus: s });
  },

  reset: () => {
    set({
      state: null,
      myDeviceId: null,
      wsStatus: "disconnected",
      stateReceivedAt: 0,
    });
  },
}));

/** Selector helper: am I currently in the active outputs set? */
export function selectIsMyOutput(s: PlayerStore): boolean {
  if (s.state === null || s.myDeviceId === null) return false;
  return s.state.active_output_device_ids.includes(s.myDeviceId);
}

/** Selector helper: dead-reckoned current ambient position in ms.
 *  When playing, advances by wall-clock time since the last state was
 *  received. When paused, returns whatever the server last said. */
export function selectAmbientPositionMs(s: PlayerStore): number {
  if (s.state === null) return 0;
  const base = s.state.ambient.position_ms;
  if (!s.state.is_playing || s.state.interrupt !== null) return base;
  const elapsed = Date.now() - s.stateReceivedAt;
  return base + Math.max(0, elapsed);
}

/** Whichever lane is currently playing — interrupt wins over ambient. */
export function selectActiveTrackId(s: PlayerStore): number | null {
  if (s.state === null) return null;
  return s.state.interrupt?.current_track_id ?? s.state.ambient.current_track_id ?? null;
}
