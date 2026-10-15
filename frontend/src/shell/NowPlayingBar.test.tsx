import { render, screen } from "@testing-library/react";
import { MemoryRouter } from "react-router-dom";
import { afterEach, describe, expect, it, vi } from "vitest";

import type * as ApiModule from "@/core/api";
import { usePlayerStore } from "@/core/playerStore";
import type { PlayerState } from "@/core/types";

import { NowPlayingBar } from "./NowPlayingBar";

vi.mock("@/core/api", async (importOriginal) => {
  const actual = await importOriginal<typeof ApiModule>();
  return {
    ...actual,
    libraryApi: { ...actual.libraryApi, getTrack: vi.fn(() => new Promise(() => {})) },
  };
});

const playing: PlayerState = {
  revision: 1,
  position_epoch: 1,
  is_playing: true,
  active_mode_id: null,
  active_output_device_ids: [],
  default_device_volume: 1,
  device_volumes: {},
  active_soundboard_id: null,
  active_preset_ids: [],
  preset_revision: 0,
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

describe("NowPlayingBar", () => {
  afterEach(() => {
    vi.restoreAllMocks();
  });

  it("renders a playing track while the clock advances between reads", () => {
    // A store selector that reads the clock returns a new value on every
    // call, which makes React re-render until it gives up.
    let now = 1_000_000;
    vi.spyOn(Date, "now").mockImplementation(() => (now += 1));
    usePlayerStore.setState({ state: playing, stateReceivedAt: now });

    render(
      <MemoryRouter>
        <NowPlayingBar />
      </MemoryRouter>,
    );

    expect(screen.getByRole("button", { name: /pause/i })).toBeInTheDocument();
  });
});
