import { act, render, waitFor } from "@testing-library/react";
import { beforeEach, describe, expect, it, vi } from "vitest";

import { presetsApi } from "@/core/api";
import type * as ApiModule from "@/core/api";
import { AudioEngine } from "@/core/audioEngine";
import { usePlayerStore } from "@/core/playerStore";
import type { PlayerState, PresetManifest } from "@/core/types";

vi.mock("@/core/api", async (importOriginal) => {
  const actual = await importOriginal<typeof ApiModule>();
  return {
    ...actual,
    presetsApi: { list: vi.fn() },
  };
});

vi.mock("@/core/playbackEngine", () => ({
  playbackEngine: {
    applyState: vi.fn(),
    destroy: vi.fn(),
    fireSfx: vi.fn(),
    setAmbientElements: vi.fn(),
    setClientId: vi.fn(),
    setHandlers: vi.fn(),
    setInterruptElement: vi.fn(),
    setPresets: vi.fn(),
    unlock: vi.fn(),
  },
}));

const preset: PresetManifest = {
  id: "cave",
  name: "Cave",
  effects: [{ type: "lowpass", frequency: 800 }],
};

function playerState(presetRevision: number): PlayerState {
  return {
    revision: presetRevision + 1,
    position_epoch: 0,
    is_playing: true,
    volume: 1,
    active_mode_id: "dnd",
    active_output_device_ids: [],
    default_device_volume: 1,
    device_volumes: {},
    active_soundboard_id: null,
    active_preset_ids: ["cave"],
    preset_revision: presetRevision,
    crossfade_ms: 0,
    crossfade_type: "linear",
    ambient: {
      current_track_id: 1,
      queue: [],
      history: [],
      position_ms: 0,
      position_anchored_at: null,
      loop: "off",
      shuffle: "off",
      source_playlist_id: null,
    },
    interrupt: null,
    looping_sfx: [],
    last_position_report: null,
    connected_devices: [],
  };
}

describe("AudioEngine preset cache", () => {
  beforeEach(() => {
    vi.mocked(presetsApi.list).mockReset().mockResolvedValue([preset]);
    usePlayerStore.setState({ state: null, myDeviceId: null });
  });

  it("refetches active manifests when preset_revision changes", async () => {
    render(<AudioEngine />);

    act(() => usePlayerStore.setState({ state: playerState(1) }));
    await waitFor(() => expect(presetsApi.list).toHaveBeenCalledTimes(1));

    act(() => usePlayerStore.setState({ state: playerState(2) }));
    await waitFor(() => expect(presetsApi.list).toHaveBeenCalledTimes(2));
  });
});
