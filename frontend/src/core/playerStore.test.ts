import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";

import {
  selectActivePositionMs,
  selectAmbientPositionMs,
  usePlayerStore,
} from "@/core/playerStore";
import type { AmbientState, InterruptState, PlayerState } from "@/core/types";

function makeAmbient(overrides: Partial<AmbientState> = {}): AmbientState {
  return {
    current_track_id: 1,
    queue: [],
    history: [],
    position_ms: 10_000,
    loop: "off",
    shuffle: "off",
    source_playlist_id: null,
    ...overrides,
  };
}

function makeInterrupt(overrides: Partial<InterruptState> = {}): InterruptState {
  return {
    current_track_id: 2,
    queue: [],
    position_ms: 3_000,
    return_to_ambient: true,
    fade_in_ms: 0,
    fade_out_ms: 0,
    duck_to: null,
    ...overrides,
  };
}

function makeState(overrides: Partial<PlayerState> = {}): PlayerState {
  return {
    revision: 1,
    position_epoch: 1,
    is_playing: true,
    volume: 1,
    active_mode_id: null,
    active_output_device_ids: [],
    device_volumes: {},
    active_soundboard_id: null,
    active_preset_ids: [],
    crossfade_ms: 0,
    crossfade_type: "linear",
    ambient: makeAmbient(),
    interrupt: null,
    looping_sfx: [],
    last_position_report: null,
    connected_devices: [],
    ...overrides,
  };
}

/** Seed the store as if a broadcast arrived at the current (fake) time. */
function receiveState(state: PlayerState | null): void {
  usePlayerStore.setState({ state, stateReceivedAt: Date.now() });
}

beforeEach(() => {
  vi.useFakeTimers();
  vi.setSystemTime(1_000_000);
  usePlayerStore.getState().reset();
});

afterEach(() => {
  vi.useRealTimers();
});

describe("selectActivePositionMs", () => {
  it("returns 0 with no state", () => {
    expect(selectActivePositionMs(usePlayerStore.getState())).toBe(0);
  });

  it("dead-reckons the ambient lane while playing, same as the ambient selector", () => {
    receiveState(makeState());
    vi.advanceTimersByTime(4_000);
    const s = usePlayerStore.getState();
    expect(selectActivePositionMs(s)).toBe(14_000);
    expect(selectActivePositionMs(s)).toBe(selectAmbientPositionMs(s));
  });

  it("freezes the ambient lane when paused", () => {
    receiveState(makeState({ is_playing: false }));
    vi.advanceTimersByTime(4_000);
    expect(selectActivePositionMs(usePlayerStore.getState())).toBe(10_000);
  });

  it("dead-reckons the interrupt lane while an interrupt is active", () => {
    receiveState(makeState({ interrupt: makeInterrupt() }));
    vi.advanceTimersByTime(2_500);
    // Interrupt base (3s) + elapsed — NOT the frozen ambient position (10s),
    // which is what the bar showed before this selector existed.
    expect(selectActivePositionMs(usePlayerStore.getState())).toBe(5_500);
  });

  it("keeps the interrupt clock ticking even when is_playing is false", () => {
    // Pause only freezes ambient (backend set_is_playing); the interrupt
    // lane keeps playing, so its clock must keep advancing.
    receiveState(makeState({ is_playing: false, interrupt: makeInterrupt() }));
    vi.advanceTimersByTime(2_000);
    const s = usePlayerStore.getState();
    expect(selectActivePositionMs(s)).toBe(5_000);
    // ...while the ambient selector stays frozen at its base.
    expect(selectAmbientPositionMs(s)).toBe(10_000);
  });

  it("falls back to ambient once the interrupt ends", () => {
    receiveState(makeState({ interrupt: makeInterrupt() }));
    vi.advanceTimersByTime(2_000);
    // Interrupt ends; server broadcasts ambient resuming from its frozen base.
    receiveState(makeState());
    vi.advanceTimersByTime(1_000);
    expect(selectActivePositionMs(usePlayerStore.getState())).toBe(11_000);
  });

  it("clamps negative elapsed (clock skew) to the interrupt base", () => {
    receiveState(makeState({ interrupt: makeInterrupt() }));
    usePlayerStore.setState({ stateReceivedAt: Date.now() + 60_000 });
    expect(selectActivePositionMs(usePlayerStore.getState())).toBe(3_000);
  });
});
