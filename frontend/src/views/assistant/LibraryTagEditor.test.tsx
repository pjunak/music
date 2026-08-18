import { render, screen, waitFor } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { beforeEach, describe, expect, it, vi } from "vitest";

import type * as ApiModule from "@/core/api";
import type {
  LibraryTagPage,
  LibraryTagTrack,
  ManualTagCatalog,
} from "@/core/api";

vi.mock("@/core/api", async (importActual) => {
  const actual = await importActual<typeof ApiModule>();
  return {
    ...actual,
    assistantApi: {
      ...actual.assistantApi,
      getManualTagCatalog: vi.fn(),
      listLibraryTags: vi.fn(),
      patchManualTags: vi.fn(),
    },
  };
});

vi.mock("@/core/toast", () => ({
  toast: {
    success: vi.fn(),
    error: vi.fn(),
  },
}));

import { assistantApi } from "@/core/api";
import { toast } from "@/core/toast";

import { LibraryTagEditor } from "./LibraryTagEditor";

const catalog: ManualTagCatalog = {
  starter_groups: [
    { key: "setting", label: "Setting", tags: ["medieval", "tavern"] },
    { key: "scene", label: "Scene", tags: ["dancing", "combat"] },
  ],
  used_tags: ["medieval"],
};

const track: LibraryTagTrack = {
  track_id: 7,
  path: "DND/Tavern Dance.flac",
  title: "Tavern Dance",
  display_title: "",
  artist: "Minstrel",
  album: "Campaign Music",
  manual_tags: ["medieval"],
  analysis_analyzer: "local-metadata/v1",
  analysis_tags: ["tavern", "festive"],
  analysis_confidence: "medium",
};

const page: LibraryTagPage = {
  items: [track],
  total: 1,
  offset: 0,
  limit: 50,
};

beforeEach(() => {
  vi.clearAllMocks();
  vi.mocked(assistantApi.getManualTagCatalog).mockResolvedValue(catalog);
  vi.mocked(assistantApi.listLibraryTags).mockResolvedValue(page);
});

describe("LibraryTagEditor", () => {
  it("keeps editable manual tags separate from read-only analysis tags", async () => {
    render(<LibraryTagEditor />);

    expect(await screen.findByRole("heading", { name: "Tavern Dance" })).toBeInTheDocument();
    expect(screen.getByText("Your tags")).toBeInTheDocument();
    expect(screen.getByText("Analysis / AI tags")).toBeInTheDocument();
    expect(screen.getByText("festive")).toBeInTheDocument();
    expect(
      await screen.findByRole("button", { name: "Remove tag medieval" }),
    ).toBeInTheDocument();
  });

  it("adds starter and custom tags and saves only the delta", async () => {
    const updated: LibraryTagTrack = {
      ...track,
      manual_tags: ["boss room", "medieval", "tavern"],
    };
    vi.mocked(assistantApi.patchManualTags).mockResolvedValue(updated);
    const user = userEvent.setup();
    render(<LibraryTagEditor />);

    await screen.findByRole("heading", { name: "Tavern Dance" });
    await screen.findByRole("button", { name: "Remove tag medieval" });
    await user.click(screen.getByRole("button", { name: "tavern" }));
    await user.type(screen.getByLabelText("Create custom tags"), "Boss Room");
    await user.click(screen.getByRole("button", { name: "Add" }));
    await user.click(screen.getByRole("button", { name: "Save manual tags" }));

    await waitFor(() =>
      expect(assistantApi.patchManualTags).toHaveBeenCalledWith(
        7,
        ["boss room", "tavern"],
        [],
      ),
    );
    expect(toast.success).toHaveBeenCalledWith(
      "Manual tags saved",
      "Playlist suggestions use them immediately.",
    );
  });

  it("removes an existing tag without changing generated tags", async () => {
    const updated: LibraryTagTrack = { ...track, manual_tags: [] };
    vi.mocked(assistantApi.patchManualTags).mockResolvedValue(updated);
    const user = userEvent.setup();
    render(<LibraryTagEditor />);

    await screen.findByRole("heading", { name: "Tavern Dance" });
    await user.click(await screen.findByRole("button", { name: "Remove tag medieval" }));
    await user.click(screen.getByRole("button", { name: "Save manual tags" }));

    await waitFor(() =>
      expect(assistantApi.patchManualTags).toHaveBeenCalledWith(7, [], ["medieval"]),
    );
    expect(screen.getByText("festive")).toBeInTheDocument();
  });
});
