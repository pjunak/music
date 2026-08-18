import { render, screen, waitFor, within } from "@testing-library/react";
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
      patchManualTagsBulk: vi.fn(),
      renameManualTag: vi.fn(),
      reviewAnalysisTag: vi.fn(),
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
  tag_usage: [{ tag: "medieval", track_count: 1 }],
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
  analysis_suggestions: [
    {
      tag: "tavern",
      analyzer_id: "local-metadata/v1",
      source_signature: "a".repeat(64),
      confidence: "medium",
      evidence: ["Mood metadata: tavern, festive"],
      status: "pending",
    },
    {
      tag: "festive",
      analyzer_id: "local-metadata/v1",
      source_signature: "a".repeat(64),
      confidence: "medium",
      evidence: ["Mood metadata: tavern, festive"],
      status: "rejected",
    },
  ],
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
  it("keeps editable manual tags separate from reviewable analysis tags", async () => {
    render(<LibraryTagEditor />);

    expect(await screen.findByRole("heading", { name: "Tavern Dance" })).toBeInTheDocument();
    expect(screen.getByText("Your tags")).toBeInTheDocument();
    expect(screen.getByText("Analysis suggestions")).toBeInTheDocument();
    expect(screen.getByText("festive")).toBeInTheDocument();
    expect(
      screen.getByRole("button", { name: "Accept tavern as manual tag" }),
    ).toBeInTheDocument();
    expect(screen.getByText("Rejected")).toBeInTheDocument();
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

  it("accepts one generated suggestion into manual tags", async () => {
    vi.mocked(assistantApi.reviewAnalysisTag).mockResolvedValue({
      track_id: 7,
      tag: "tavern",
      analyzer_id: "local-metadata/v1",
      source_signature: "a".repeat(64),
      decision: "accepted",
      manual_tags: ["medieval", "tavern"],
    });
    const user = userEvent.setup();
    render(<LibraryTagEditor />);

    await screen.findByRole("heading", { name: "Tavern Dance" });
    await user.click(
      screen.getByRole("button", { name: "Accept tavern as manual tag" }),
    );

    await waitFor(() =>
      expect(assistantApi.reviewAnalysisTag).toHaveBeenCalledWith(
        7,
        track.analysis_suggestions[0],
        "accepted",
      ),
    );
    expect(
      screen.getByRole("button", { name: "Remove tag tavern" }),
    ).toBeInTheDocument();
    expect(screen.getByText("Accepted")).toBeInTheDocument();
    expect(toast.success).toHaveBeenCalledWith(
      "Tag added",
      "“tavern” is now one of your manual tags.",
    );
  });

  it("reopens a rejected suggestion without removing manual tags", async () => {
    vi.mocked(assistantApi.reviewAnalysisTag).mockResolvedValue({
      track_id: 7,
      tag: "festive",
      analyzer_id: "local-metadata/v1",
      source_signature: "a".repeat(64),
      decision: "pending",
      manual_tags: ["medieval"],
    });
    const user = userEvent.setup();
    render(<LibraryTagEditor />);

    await screen.findByRole("heading", { name: "Tavern Dance" });
    await user.click(screen.getByRole("button", { name: "Review festive again" }));

    await waitFor(() =>
      expect(assistantApi.reviewAnalysisTag).toHaveBeenCalledWith(
        7,
        track.analysis_suggestions[1],
        "pending",
      ),
    );
    expect(
      screen.getByRole("button", { name: "Accept festive as manual tag" }),
    ).toBeInTheDocument();
    expect(
      screen.getByRole("button", { name: "Remove tag medieval" }),
    ).toBeInTheDocument();
  });

  it("applies chosen tags to a multi-track selection", async () => {
    vi.mocked(assistantApi.patchManualTagsBulk).mockResolvedValue({
      requested_tracks: 1,
      matched_tracks: 1,
      changed_track_ids: [7],
      missing_track_ids: [],
      failures: [],
    });
    const user = userEvent.setup();
    render(<LibraryTagEditor />);

    await screen.findByRole("heading", { name: "Tavern Dance" });
    await user.click(
      screen.getByRole("checkbox", { name: "Select Tavern Dance for bulk tagging" }),
    );
    const bulkEditor = screen.getByRole("region", { name: "Bulk tag editor" });
    await user.click(within(bulkEditor).getByRole("button", { name: "dancing" }));
    await user.click(within(bulkEditor).getByRole("button", { name: "Add to selected" }));

    await waitFor(() =>
      expect(assistantApi.patchManualTagsBulk).toHaveBeenCalledWith([7], ["dancing"], []),
    );
    expect(toast.success).toHaveBeenCalledWith("Tags added", "1 track was updated.");
  });
});
