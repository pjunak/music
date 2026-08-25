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
      reviewAnalysisTagsBulk: vi.fn(),
    },
  };
});

vi.mock("@/components/confirmDialog", () => ({
  confirmDialog: vi.fn(),
}));

vi.mock("@/core/toast", () => ({
  toast: {
    success: vi.fn(),
    error: vi.fn(),
  },
}));

import { assistantApi } from "@/core/api";
import { toast } from "@/core/toast";
import { confirmDialog } from "@/components/confirmDialog";

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
  audio_signal: {
    analyzer_id: "local-audio/v1",
    confidence: "medium",
    evidence: [
      "Signal level: -18.0 dBFS RMS, -2.0 dBFS peak",
      "Signal proxies do not identify instruments, genre, setting, period, scene, or mood.",
    ],
    metrics: {
      schema: "local-audio/v1",
      rms_dbfs: -18,
      level_spread_db: 7.5,
      high_frequency_ratio: 0.21,
      tempo_bpm: 120,
    },
  },
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
  vi.mocked(confirmDialog).mockResolvedValue(true);
});

describe("LibraryTagEditor", () => {
  it("keeps editable manual tags separate from reviewable analysis tags", async () => {
    render(<LibraryTagEditor />);

    expect(await screen.findByRole("heading", { name: "Tavern Dance" })).toBeInTheDocument();
    expect(screen.getByText("Your tags")).toBeInTheDocument();
    expect(screen.getByText("Generated suggestions")).toBeInTheDocument();
    expect(screen.getByText("Audio signal evidence")).toBeInTheDocument();
    expect(screen.getByText("120.0 BPM")).toBeInTheDocument();
    expect(screen.getByText("festive")).toBeInTheDocument();
    expect(
      screen.getByRole("button", { name: "Accept tavern into mood library" }),
    ).toBeInTheDocument();
    expect(screen.getByText("Rejected")).toBeInTheDocument();
    expect(
      await screen.findByRole("button", { name: "Remove tag medieval" }),
    ).toBeInTheDocument();
  });

  it("labels connected-model suggestions separately from manual tags", async () => {
    vi.mocked(assistantApi.listLibraryTags).mockResolvedValue({
      ...page,
      items: [
        {
          ...track,
          analysis_suggestions: [
            {
              ...track.analysis_suggestions[0],
              tag: "dancing",
              analyzer_id: "model-context-tagger/v6",
              evidence: ["Title and genre support a dancing scene."],
            },
          ],
        },
      ],
    });
    render(<LibraryTagEditor />);

    expect(
      await screen.findByText(/model-context-tagger\/v6/),
    ).toBeInTheDocument();
    expect(
      screen.getByRole("button", { name: "Accept dancing into mood library" }),
    ).toBeInTheDocument();
    expect(screen.getByText("Your tags")).toBeInTheDocument();
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
    await user.click(screen.getByRole("button", { name: "Save mood tags" }));

    await waitFor(() =>
      expect(assistantApi.patchManualTags).toHaveBeenCalledWith(
        7,
        ["boss room", "tavern"],
        [],
      ),
    );
    expect(toast.success).toHaveBeenCalledWith(
      "Mood tags saved",
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
    await user.click(screen.getByRole("button", { name: "Save mood tags" }));

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
      screen.getByRole("button", { name: "Accept tavern into mood library" }),
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
      "“tavern” is now in your mood library.",
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
      screen.getByRole("button", { name: "Accept festive into mood library" }),
    ).toBeInTheDocument();
    expect(
      screen.getByRole("button", { name: "Remove tag medieval" }),
    ).toBeInTheDocument();
  });

  it("filters the library to tracks with pending analysis review", async () => {
    const user = userEvent.setup();
    render(<LibraryTagEditor />);

    await screen.findByRole("heading", { name: "Tavern Dance" });
    await user.selectOptions(
      screen.getByLabelText("Filter analysis review"),
      "pending",
    );

    await waitFor(() =>
      expect(assistantApi.listLibraryTags).toHaveBeenLastCalledWith({
        review: "pending",
        offset: 0,
        limit: 50,
      }),
    );
  });

  it("applies an explicit bulk decision to selected suggestions", async () => {
    vi.mocked(assistantApi.reviewAnalysisTagsBulk).mockResolvedValue({
      requested_items: 1,
      applied: [
        {
          track_id: 7,
          tag: "tavern",
          analyzer_id: "local-metadata/v1",
          source_signature: "a".repeat(64),
          decision: "accepted",
        },
      ],
      failures: [],
    });
    const user = userEvent.setup();
    render(<LibraryTagEditor />);

    await screen.findByRole("heading", { name: "Tavern Dance" });
    await user.click(
      screen.getByRole("checkbox", {
        name: "Select tavern suggestion for bulk review",
      }),
    );
    const bulkReview = screen.getByRole("region", {
      name: "Bulk analysis review",
    });
    await user.click(
      within(bulkReview).getByRole("button", {
        name: "Add selected to my tags",
      }),
    );

    expect(confirmDialog).toHaveBeenCalledWith({
      title: "Add selected suggestions to your tags?",
      body: "1 selected suggestion will be copied into your mood library.",
      confirmLabel: "Add selected tags",
    });
    await waitFor(() =>
      expect(assistantApi.reviewAnalysisTagsBulk).toHaveBeenCalledWith(
        [
          {
            track_id: 7,
            tag: "tavern",
            analyzer_id: "local-metadata/v1",
            source_signature: "a".repeat(64),
          },
        ],
        "accepted",
      ),
    );
    expect(toast.success).toHaveBeenCalledWith(
      "Suggestions accepted",
      "1 decision was saved.",
    );
  });

  it("reports skipped suggestions after a partial bulk decision", async () => {
    const bothPending: LibraryTagTrack = {
      ...track,
      analysis_suggestions: track.analysis_suggestions.map((suggestion) => ({
        ...suggestion,
        status: "pending",
      })),
    };
    vi.mocked(assistantApi.listLibraryTags).mockResolvedValue({
      ...page,
      items: [bothPending],
    });
    vi.mocked(assistantApi.reviewAnalysisTagsBulk).mockResolvedValue({
      requested_items: 2,
      applied: [
        {
          track_id: 7,
          tag: "tavern",
          analyzer_id: "local-metadata/v1",
          source_signature: "a".repeat(64),
          decision: "rejected",
        },
      ],
      failures: [
        {
          track_id: 7,
          tag: "festive",
          analyzer_id: "local-metadata/v1",
          source_signature: "a".repeat(64),
          code: "stale",
          error: "Analysis changed",
        },
      ],
    });
    const user = userEvent.setup();
    render(<LibraryTagEditor />);

    await screen.findByRole("heading", { name: "Tavern Dance" });
    await user.click(
      screen.getByRole("checkbox", {
        name: "Select tavern suggestion for bulk review",
      }),
    );
    await user.click(
      screen.getByRole("checkbox", {
        name: "Select festive suggestion for bulk review",
      }),
    );
    await user.click(
      within(screen.getByRole("region", { name: "Bulk analysis review" })).getByRole(
        "button",
        { name: "Reject selected" },
      ),
    );

    await waitFor(() =>
      expect(toast.error).toHaveBeenCalledWith(
        "Bulk review partly applied",
        "1 applied; 1 skipped. #7 “festive”: Analysis changed",
      ),
    );
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
