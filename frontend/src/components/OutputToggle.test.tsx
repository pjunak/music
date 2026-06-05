import { render, screen, waitFor } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";

import type * as ApiModule from "@/core/api";
import type { PlayerState } from "@/core/types";

// Mock the side-effecting collaborators. Keep the real `@/core/api` module
// (auth.ts and others import ApiError/api from it) but stub devicesApi.save.
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

import { devicesApi } from "@/core/api";
import { useAuthStore } from "@/core/auth";
import { usePlayerStore } from "@/core/playerStore";
import { toast } from "@/core/toast";
import { useUiStore } from "@/core/uiStore";
import { wsClient } from "@/core/ws";

import { OutputToggle } from "./OutputToggle";

const CLIENT_ID = "cid-test";

function seedStores(opts: { designated: boolean; active?: boolean }) {
  useAuthStore.setState({ status: "authenticated", user: { id: 1, username: "dm" } });
  useUiStore.setState({
    clientId: CLIENT_ID,
    deviceName: "DM Laptop",
    forceLocalPlayback: false,
  });
  const active = opts.active ?? false;
  usePlayerStore.setState({
    myDeviceId: CLIENT_ID,
    stateReceivedAt: 1,
    state: {
      active_output_device_ids: active ? [CLIENT_ID] : [],
      connected_devices: [
        { device_id: CLIENT_ID, name: "DM Laptop", is_output: opts.designated },
      ],
    } as unknown as PlayerState,
  });
}

beforeEach(() => {
  vi.clearAllMocks();
});

afterEach(() => {
  useAuthStore.setState({ status: "unknown", user: null });
  usePlayerStore.setState({ myDeviceId: null, state: null });
});

describe("OutputToggle — authed self-designate", () => {
  it("designates THIS device, then activates, when it isn't a designated output yet", async () => {
    seedStores({ designated: false });
    render(<OutputToggle />);

    // The operator's own window is enable-able even though it isn't a
    // designated output (the regression: it used to render a dead label).
    const btn = screen.getByRole("button", { name: /play on this device/i });
    expect(btn).toHaveTextContent("Output OFF");

    await userEvent.click(btn);

    // First the explicit (one-click) designation...
    await waitFor(() =>
      expect(devicesApi.save).toHaveBeenCalledWith(CLIENT_ID, {
        name: "DM Laptop",
        is_output: true,
      }),
    );
    // ...then activation over the WS.
    await waitFor(() =>
      expect(wsClient.send).toHaveBeenCalledWith({
        type: "set_active_outputs",
        device_ids: [CLIENT_ID],
      }),
    );
    // ...and a one-time toast surfacing the persistent side-effect.
    expect(toast.success).toHaveBeenCalled();
  });

  it("activates without re-designating when already a designated output", async () => {
    seedStores({ designated: true });
    render(<OutputToggle />);

    await userEvent.click(
      screen.getByRole("button", { name: /play on this device/i }),
    );

    expect(devicesApi.save).not.toHaveBeenCalled();
    expect(toast.success).not.toHaveBeenCalled();
    expect(wsClient.send).toHaveBeenCalledWith({
      type: "set_active_outputs",
      device_ids: [CLIENT_ID],
    });
  });

  it("does not silence on the first click while ON (arm-to-confirm)", async () => {
    seedStores({ designated: true, active: true });
    render(<OutputToggle />);

    const btn = screen.getByRole("button", { name: /stop playing here/i });
    expect(btn).toHaveTextContent("Output ON");

    await userEvent.click(btn);

    // One click only arms — it must NOT send the deactivation yet.
    expect(wsClient.send).not.toHaveBeenCalled();
    expect(btn).toHaveTextContent("Click again to stop");
  });
});
