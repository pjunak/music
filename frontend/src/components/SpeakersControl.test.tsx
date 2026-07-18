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
    wsStatus: "connected",
    stateReceivedAt: 1,
    state: {
      volume: 1,
      default_device_volume: 1,
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

  it("lists every connected device and activates an undesignated one", async () => {
    useAuthStore.setState({ status: "authenticated", user: { id: 1, username: "dm" } });
    useUiStore.setState({ clientId: CID, deviceName: "DM", forceLocalPlayback: false });
    usePlayerStore.setState({
      myDeviceId: CID,
      wsStatus: "connected",
      stateReceivedAt: 1,
      state: {
        volume: 1,
        default_device_volume: 1,
        active_output_device_ids: [],
        device_volumes: {},
        connected_devices: [
          { device_id: CID, name: "DM", is_output: false },
          // Connected but NOT saved/designated — must still be listed + tickable.
          { device_id: "tv-1", name: "Living Room TV", is_output: true },
        ],
      } as unknown as PlayerState,
    });
    renderControl();
    await userEvent.click(screen.getByRole("button", { name: /speakers/i }));

    // Every connected device appears, and the designated one shows "default".
    expect(screen.getByText("Living Room TV")).toBeInTheDocument();
    expect(screen.getByText("default")).toBeInTheDocument();

    const checkboxes = screen.getAllByRole("checkbox");
    expect(checkboxes).toHaveLength(2); // this device + the TV
    await userEvent.click(checkboxes[1]); // activate the TV — no PUT/designation
    expect(wsClient.send).toHaveBeenCalledWith({
      type: "set_active_outputs",
      device_ids: ["tv-1"],
    });
  });

  it("sends a per-device volume on slider change", async () => {
    seedAuthed({ designated: true });
    renderControl();
    await userEvent.click(screen.getByRole("button", { name: /speakers/i }));
    fireEvent.change(screen.getByLabelText("DM volume"), {
      target: { value: "0.4" },
    });
    expect(wsClient.send).toHaveBeenCalledWith({
      type: "set_device_volume",
      device_id: CID,
      volume: 0.4,
    });
  });

  it("renders another device's canonical server volume", async () => {
    seedAuthed({ designated: true });
    usePlayerStore.setState((current) => ({
      state: current.state
        ? {
            ...current.state,
            device_volumes: { "tv-1": 0.4 },
            connected_devices: [
              ...current.state.connected_devices,
              {
                device_id: "tv-1",
                client_id: "tv-1",
                name: "Living Room TV",
                is_output: true,
              },
            ],
          }
        : null,
    }));
    renderControl();
    await userEvent.click(screen.getByRole("button", { name: /speakers/i }));
    expect(screen.getByLabelText("Living Room TV volume")).toHaveValue("0.4");
  });

  it("renders and submits effective volume for a legacy server", async () => {
    seedAuthed({ designated: true });
    usePlayerStore.setState((current) => {
      if (current.state === null) return { state: null };
      const state = {
        ...current.state,
        volume: 0.2,
        device_volumes: { [CID]: 0.5 },
      };
      delete state.default_device_volume;
      return { state };
    });
    renderControl();
    await userEvent.click(screen.getByRole("button", { name: /speakers/i }));
    const slider = screen.getByLabelText("DM volume");
    expect(slider).toHaveValue("0.1");

    fireEvent.change(slider, { target: { value: "0.05" } });
    expect(wsClient.send).toHaveBeenCalledWith({
      type: "set_device_volume",
      device_id: CID,
      volume: 0.25,
    });
  });

  it("raises a legacy master and preserves other effective device levels", async () => {
    seedAuthed({ designated: true });
    usePlayerStore.setState((current) => {
      if (current.state === null) return { state: null };
      const state = {
        ...current.state,
        volume: 0.2,
        device_volumes: { [CID]: 0.5, "tv-1": 1 },
        connected_devices: [
          ...current.state.connected_devices,
          {
            device_id: "tv-1",
            client_id: "tv-1",
            name: "TV",
            is_output: true,
          },
        ],
      };
      delete state.default_device_volume;
      return { state };
    });
    renderControl();
    await userEvent.click(screen.getByRole("button", { name: /speakers/i }));

    fireEvent.change(screen.getByLabelText("DM volume"), {
      target: { value: "0.8" },
    });

    expect(wsClient.send).toHaveBeenNthCalledWith(1, {
      type: "set_volume",
      volume: 0.8,
    });
    expect(wsClient.send).toHaveBeenCalledWith({
      type: "set_device_volume",
      device_id: CID,
      volume: 1,
    });
    expect(wsClient.send).toHaveBeenCalledWith({
      type: "set_device_volume",
      device_id: "tv-1",
      volume: 0.25,
    });
  });

  it("disables device controls while disconnected", async () => {
    seedAuthed({ designated: true });
    usePlayerStore.setState({ wsStatus: "disconnected" });
    renderControl();
    await userEvent.click(screen.getByRole("button", { name: /speakers/i }));
    expect(screen.getByRole("slider", { name: "DM volume" })).toBeDisabled();
    expect(screen.getByRole("checkbox", { name: "DM output" })).toBeDisabled();
  });

  it("marks this device with a subtle visual, not '(this)' text", async () => {
    seedAuthed({ designated: false });
    renderControl();
    await userEvent.click(screen.getByRole("button", { name: /speakers/i }));
    // The screen-reader marker is present; the old "(this)" suffix is gone.
    expect(screen.getByText("This device")).toBeInTheDocument();
    expect(screen.queryByText(/\(this\)/)).toBeNull();
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
