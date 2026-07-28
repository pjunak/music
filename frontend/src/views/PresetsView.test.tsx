import { fireEvent, render, screen, waitFor } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";

import type * as ApiModule from "@/core/api";
import type { PlayerState, PresetManifest } from "@/core/types";

vi.mock("@/core/ws", () => ({ wsClient: { send: vi.fn() } }));
vi.mock("@/core/api", async (importActual) => {
  const actual = await importActual<typeof ApiModule>();
  return {
    ...actual,
    presetsApi: { ...actual.presetsApi, list: vi.fn() },
    presetsAdminApi: {
      ...actual.presetsAdminApi,
      update: vi.fn(),
    },
  };
});

import { presetsAdminApi, presetsApi } from "@/core/api";
import { usePlayerStore } from "@/core/playerStore";
import { wsClient } from "@/core/ws";

import { PresetsView } from "./PresetsView";

const cave: PresetManifest = {
  id: "cave",
  name: "Cave",
  effects: [{ type: "lowpass", frequency: 800, q: 0.7 }],
};

beforeEach(() => {
  vi.clearAllMocks();
  vi.mocked(presetsApi.list).mockResolvedValue([cave]);
  vi.mocked(presetsAdminApi.update).mockResolvedValue(cave);
  usePlayerStore.setState({
    state: {
      active_mode_id: "dnd",
      active_preset_ids: ["hall"],
    } as unknown as PlayerState,
  });
});

afterEach(() => {
  usePlayerStore.setState({ state: null });
});

describe("Preset live tuning", () => {
  it("activates the selected preset and auto-saves effect changes", async () => {
    render(<PresetsView />);
    await userEvent.click(await screen.findByRole("button", { name: /Cave/ }));

    fireEvent.click(screen.getByRole("checkbox", { name: "Live tuning" }));
    expect(wsClient.send).toHaveBeenCalledWith({
      type: "set_active_presets",
      preset_ids: ["hall", "cave"],
    });
    await waitFor(() => expect(presetsAdminApi.update).toHaveBeenCalledTimes(1));

    vi.mocked(presetsAdminApi.update).mockClear();
    fireEvent.click(screen.getByRole("checkbox", { name: "Enable Muffle (low-pass)" }));

    await waitFor(() => expect(presetsAdminApi.update).toHaveBeenCalledTimes(1));
    expect(presetsAdminApi.update).toHaveBeenCalledWith(
      "dnd",
      "cave",
      expect.objectContaining({ effects: [] }),
    );
    expect(screen.getByRole("status")).toHaveTextContent("Applied");

    fireEvent.click(screen.getByRole("checkbox", { name: "Live tuning" }));
    expect(wsClient.send).toHaveBeenLastCalledWith({
      type: "set_active_presets",
      preset_ids: ["hall"],
    });
  });
});
