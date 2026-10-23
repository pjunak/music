import { render, screen } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { beforeEach, describe, expect, it, vi } from "vitest";

import type * as ApiModule from "@/core/api";
import type { ModeSummary } from "@/core/types";

vi.mock("@/core/ws", () => ({ wsClient: { send: vi.fn() } }));
vi.mock("@/core/toast", () => ({
  toast: { error: vi.fn(), success: vi.fn() },
}));
vi.mock("@/core/api", async (importActual) => {
  const actual = await importActual<typeof ApiModule>();
  return {
    ...actual,
    modesAdminApi: { ...actual.modesAdminApi, update: vi.fn() },
  };
});

import { modesAdminApi } from "@/core/api";

import { ModeManagerModal } from "./ModeManagerModal";

function mode(multipleOutputs: boolean): ModeSummary {
  return {
    id: "table",
    name: "Table",
    panels: [],
    playlist_categories: [],
    has_theme: false,
    default_crossfade_ms: 0,
    default_soundboard: null,
    multiple_outputs: multipleOutputs,
  };
}

beforeEach(() => vi.clearAllMocks());

describe("ModeManagerModal", () => {
  it("saves whether a mode plays on several speakers", async () => {
    vi.mocked(modesAdminApi.update).mockResolvedValue(mode(true));
    const onChanged = vi.fn();
    render(
      <ModeManagerModal
        open
        onClose={vi.fn()}
        modes={[mode(false)]}
        activeModeId="table"
        onChanged={onChanged}
      />,
    );
    const option = screen.getByRole("checkbox", {
      name: "Play on several speakers at once",
    });
    expect(option).not.toBeChecked();
    await userEvent.click(option);

    expect(modesAdminApi.update).toHaveBeenCalledWith("table", {
      multiple_outputs: true,
    });
    expect(onChanged).toHaveBeenCalled();
  });
});
