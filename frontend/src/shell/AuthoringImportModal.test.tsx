import { render, screen, waitFor, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { beforeEach, describe, expect, it, vi } from "vitest";

import type * as ApiModule from "@/core/api";
import type { ModeSummary } from "@/core/types";

vi.mock("@/core/api", async (importActual) => {
  const actual = await importActual<typeof ApiModule>();
  return {
    ...actual,
    modesApi: { ...actual.modesApi, list: vi.fn() },
    authoringImportApi: {
      preview: vi.fn(),
      commit: vi.fn(),
    },
  };
});

vi.mock("@/core/toast", () => ({
  toast: {
    success: vi.fn(),
    warn: vi.fn(),
  },
}));

import { authoringImportApi, modesApi } from "@/core/api";
import { toast } from "@/core/toast";

import { AuthoringImportModal } from "./AuthoringImportModal";

function mode(id: string, name: string): ModeSummary {
  return {
    id,
    name,
    panels: [],
    playlist_categories: [],
    has_theme: false,
    default_crossfade_ms: 1200,
    default_soundboard: null,
  };
}

const preview = {
  source_mode: { id: "source", name: "Source mode" },
  target_mode: { id: "target", name: "Target mode" },
  items: [
    {
      kind: "preset" as const,
      resource_id: "cave",
      name: "Cave",
      summary: "1 effect",
      status: "ready" as const,
      reason: null,
    },
    {
      kind: "cue" as const,
      resource_id: "arrival",
      name: "Arrival",
      summary: "2 actions",
      status: "ready" as const,
      reason: null,
    },
    {
      kind: "playlist" as const,
      resource_id: "1",
      name: "Town",
      summary: "4 tracks",
      status: "conflict" as const,
      reason: "A playlist named Town already exists in the target mode.",
    },
  ],
};

beforeEach(() => {
  vi.clearAllMocks();
  vi.mocked(modesApi.list).mockResolvedValue([
    mode("target", "Target mode"),
    mode("source", "Source mode"),
  ]);
  vi.mocked(authoringImportApi.preview).mockResolvedValue(preview);
  vi.mocked(authoringImportApi.commit).mockResolvedValue({
    imported: [preview.items[0]],
    skipped: [],
    missing_track_paths: [],
  });
});

describe("AuthoringImportModal", () => {
  it("previews create-only conflicts and commits only the selected resources", async () => {
    const user = userEvent.setup();
    const onClose = vi.fn();
    const onImported = vi.fn();

    render(
      <AuthoringImportModal
        open
        targetModeId="target"
        onClose={onClose}
        onImported={onImported}
      />,
    );

    expect(await screen.findByRole("combobox", { name: "Source mode" })).toHaveValue(
      "source",
    );
    await waitFor(() =>
      expect(authoringImportApi.preview).toHaveBeenCalledWith("source", "target"),
    );

    const direction = await screen.findByLabelText("Import direction");
    expect(within(direction).getByText("Source mode")).toBeInTheDocument();
    expect(within(direction).getByText("Target mode")).toBeInTheDocument();
    expect(screen.getByRole("checkbox", { name: /Cave/ })).toBeChecked();
    expect(screen.getByRole("checkbox", { name: /Arrival/ })).toBeChecked();
    expect(screen.getByRole("checkbox", { name: /Town/ })).toBeDisabled();
    expect(screen.getByRole("button", { name: "Import 2 items" })).toBeEnabled();

    await user.click(screen.getByRole("checkbox", { name: /Arrival/ }));
    await user.click(screen.getByRole("button", { name: "Import 1 item" }));

    await waitFor(() =>
      expect(authoringImportApi.commit).toHaveBeenCalledWith("source", "target", [
        { kind: "preset", resource_id: "cave" },
      ]),
    );
    expect(toast.success).toHaveBeenCalledWith("Authoring imported", "1 item imported");
    expect(onImported).toHaveBeenCalledWith({
      imported: [preview.items[0]],
      skipped: [],
      missing_track_paths: [],
    });
    expect(onClose).toHaveBeenCalledOnce();
  });

  it("explains when there is no other mode to import from", async () => {
    vi.mocked(modesApi.list).mockResolvedValue([mode("target", "Target mode")]);

    render(
      <AuthoringImportModal
        open
        targetModeId="target"
        onClose={vi.fn()}
        onImported={vi.fn()}
      />,
    );

    expect(
      await screen.findByText("There is no other mode to import from. Create another mode first."),
    ).toBeInTheDocument();
    expect(authoringImportApi.preview).not.toHaveBeenCalled();
  });
});
