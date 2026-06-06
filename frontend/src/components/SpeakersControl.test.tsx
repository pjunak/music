import { fireEvent, render, screen } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { MemoryRouter } from "react-router-dom";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";

import type * as ApiModule from "@/core/api";
import type { PlayerState } from "@/core/types";

vi.mock("@/core/ws", () => ({ wsClient: { send: vi.fn() } }));
vi.mock("@/core/playbackEngine", () => ({
  playbackEngine: { unlock: vi.fn(), applyState: vi.fn() },
}));
vi.mock("@/core/toast", () => ({
  toast: { error: vi.fn(), success: vi.fn() },
}));
vi.mock("@/core/api", async (importActual) => {
  const actual = await importActual<typeof ApiModule>();
  return {
    ...actual,
    devicesApi: { ...actual.devicesApi, save: vi.fn().mockResolvedValue({}) },
  };
});

import { useAuthStore } from "@/core/auth";
import { usePlayerStore } from "@/core/playerStore";
import { useUiStore } from "@/core/uiStore";
import { wsClient } from "@/core/ws";

import { SpeakersControl } from "./SpeakersControl";

const CID = "cid-test";

function seedAuthed(opts: { designated?: boolean; active?: boolean } = {}) {
  useAuthStore.setState({ status: "authenticated", user: { id: 1, username: "dm" } });
  useUiStore.setState({ clientId: CID, deviceName: "DM", forceLocalPlayback: false });
  usePlayerStore.setState({
    myDeviceId: CID,
    stateReceivedAt: 1,
    state: {
      active_output_device_ids: opts.active ? [CID] : [],
      device_volumes: {},
      connected_devices: [
        { device_id: CID, name: "DM", is_output: opts.designated ?? true },
      ],
    } as unknown as PlayerState,
  });
}

const renderControl = () =>
  render(
    <MemoryRouter>
      <SpeakersControl />
    </MemoryRouter>,
  );

beforeEach(() => vi.clearAllMocks());
afterEach(() => {
  useAuthStore.setState({ status: "unknown", user: null });
  usePlayerStore.setState({ myDeviceId: null, state: null });
});

describe("SpeakersControl", () => {
  it("opens a popover and activates this device", async () => {
    seedAuthed({ designated: true });
    renderControl();
    await userEvent.click(screen.getByRole("button", { name: /speakers/i }));
    const checkbox = screen.getByRole("checkbox");
    expect(checkbox).not.toBeChecked();
    await userEvent.click(checkbox);
    expect(wsClient.send).toHaveBeenCalledWith({
      type: "set_active_outputs",
      device_ids: [CID],
    });
  });

  it("sends a per-device volume on slider change", async () => {
    seedAuthed({ designated: true });
    renderControl();
    await userEvent.click(screen.getByRole("button", { name: /speakers/i }));
    fireEvent.change(screen.getByLabelText("DM (this) volume"), {
      target: { value: "0.4" },
    });
    expect(wsClient.send).toHaveBeenCalledWith({
      type: "set_device_volume",
      device_id: CID,
      volume: 0.4,
    });
  });

  it("gives guests a local-only toggle, no popover", () => {
    useAuthStore.setState({ status: "anonymous", user: null });
    useUiStore.setState({ clientId: CID, forceLocalPlayback: false });
    usePlayerStore.setState({
      myDeviceId: CID,
      state: {
        active_output_device_ids: [],
        device_volumes: {},
        connected_devices: [],
      } as unknown as PlayerState,
    });
    renderControl();
    expect(
      screen.getByRole("button", { name: /output off/i }),
    ).toBeInTheDocument();
  });
});
