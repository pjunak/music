import { create } from "zustand";

import type { PlayerState, WsMessage } from "@/core/types";
import { useUiStore } from "@/core/uiStore";
import type { WsStatus } from "@/core/ws";

interface PlayerStore {
  state: PlayerState | null;
  myDeviceId: string | null;
  wsStatus: WsStatus;
  /** Wall-clock timestamp (ms since epoch) when the latest state was received.
   *  Used by the player UI to dead-reckon the current playback position. */
  stateReceivedAt: number;
  /** Whether this socket generation has delivered any state-bearing frame. */
  hasStateThisConnection: boolean;

  applyMessage: (msg: WsMessage) => void;
  setStatus: (s: WsStatus) => void;
  reset: () => void;
}

export const usePlayerStore = create<PlayerStore>((set) => ({
  state: null,
  myDeviceId: null,
  wsStatus: "disconnected",
  stateReceivedAt: 0,
  hasStateThisConnection: false,

  applyMessage: (msg) => {
    if (msg.type === "state_snapshot" || msg.type === "state_changed") {
      set((current) => {
        if (
          current.hasStateThisConnection &&
          current.state !== null &&
          msg.state.revision < current.state.revision
        ) {
          return {};
        }
        return {
          state: msg.state,
          // Identity is our stable client_id; the pre-register snapshot cannot
          // carry it. Preserve it on deltas.
          ...(msg.type === "state_snapshot"
            ? { myDeviceId: useUiStore.getState().clientId }
            : {}),
          stateReceivedAt: Date.now(),
          hasStateThisConnection: true,
        };
      });
    }
    // sfx_fired / error handled by the audio engine and toast layer
    // respectively (not here — we only track PlayerState in this store).
  },

  setStatus: (s) => {
    set({
      wsStatus: s,
      ...(s === "connecting" ? { hasStateThisConnection: false } : {}),
    });
  },

  reset: () => {
    set({
      state: null,
      myDeviceId: null,
      wsStatus: "disconnected",
      stateReceivedAt: 0,
      hasStateThisConnection: false,
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
  // No ambient track loaded → there is no position to report. Without this
  // guard the clock dead-reckons from a stale position_ms whenever the
  // server reports is_playing=true with an empty ambient lane (a playlist
  // that ran off the end, a deleted track pruned on boot), which is what
  // made "Nothing playing" tick upward.
  if (s.state.ambient.current_track_id === null) return 0;
  const base = s.state.ambient.position_ms;
  if (!s.state.is_playing || s.state.interrupt !== null) return base;
  const elapsed = Date.now() - s.stateReceivedAt;
  return base + Math.max(0, elapsed);
}

/** Selector helper: dead-reckoned position of the audibly playing lane.
 *  While an interrupt is active, its clock ticks regardless of `is_playing` —
 *  pause only freezes ambient (see backend `set_is_playing`) — so we
 *  dead-reckon from the interrupt's position unconditionally. The server
 *  materializes `position_ms` in every broadcast, so the base is current at
 *  `stateReceivedAt`. With no interrupt this is the ambient position. */
export function selectActivePositionMs(s: PlayerStore): number {
  if (s.state === null) return 0;
  const interrupt = s.state.interrupt;
  if (interrupt === null) return selectAmbientPositionMs(s);
  const elapsed = Date.now() - s.stateReceivedAt;
  return interrupt.position_ms + Math.max(0, elapsed);
}

/** Whichever lane is currently playing — interrupt wins over ambient. */
export function selectActiveTrackId(s: PlayerStore): number | null {
  if (s.state === null) return null;
  return s.state.interrupt?.current_track_id ?? s.state.ambient.current_track_id ?? null;
}

/** Shared stable empty array — one reference, reused, so the `?? []` default in
 *  `usePlayerArray` never mints a fresh array (which would loop the store). */
const EMPTY_ARRAY: readonly never[] = [];

/** Stable-selector helper for array slices of `PlayerState`. Returns the
 *  selected array, or a SHARED empty array when it's nullish (e.g. before the
 *  first snapshot arrives) — so the `?? []` default lives OUTSIDE the selector
 *  and can't loop `useSyncExternalStore` to React #185.
 *
 *  Prefer this over `usePlayerStore((s) => s.state?.x ?? [])`, which mints a
 *  fresh `[]` on every call and is forbidden by the `local/stable-store-selector`
 *  ESLint rule. */
export function usePlayerArray<T>(
  selector: (s: PlayerStore) => readonly T[] | undefined,
): readonly T[] {
  return usePlayerStore(selector) ?? EMPTY_ARRAY;
}
