import { act, render, screen, waitFor } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { MemoryRouter } from "react-router-dom";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";

import type * as ApiModule from "@/core/api";
import type { AuthoringImportPreview, PlaylistSuggestion } from "@/core/api";
import { usePlayerStore } from "@/core/playerStore";
import type { PlayerState } from "@/core/types";

vi.mock("@/core/api", async (importActual) => {
  const actual = await importActual<typeof ApiModule>();
  return {
    ...actual,
    assistantApi: { suggestPlaylist: vi.fn() },
    authoringImportApi: {
      ...actual.authoringImportApi,
      previewDocument: vi.fn(),
      commitDocument: vi.fn(),
    },
  };
});

vi.mock("@/core/toast", () => ({
  toast: {
    success: vi.fn(),
    error: vi.fn(),
  },
}));

import { assistantApi, authoringImportApi } from "@/core/api";
import { toast } from "@/core/toast";

import { PlaylistBuilderView } from "./PlaylistBuilderView";

const suggestion: PlaylistSuggestion = {
  engine: "local-metadata/v1",
  library_tracks: 24,
  eligible_tracks: 22,
  intent: {
    matched_moods: ["tense", "dark"],
    search_terms: ["rainy"],
    energy: 0.48,
    brightness: 0.21,
    tension: 0.86,
  },
  candidates: [
    {
      track_id: 11,
      path: "Scores/Rainy Alley.flac",
      title: "Rainy Alley",
      display_title: "Rainy Alley",
      artist: "Tabletop Ensemble",
      album: "City Mysteries",
      origin: "Scores",
      genre: "soundtrack",
      length_s: 240,
      bpm: 92,
      match_score: 0.91,
      confidence: "high",
      reasons: ["Metadata matches: rainy", "92 BPM supports the requested pace"],
      default_selected: true,
    },
    {
      track_id: 12,
      path: "Scores/Distant Footsteps.flac",
      title: "Distant Footsteps",
      display_title: "Distant Footsteps",
      artist: "Tabletop Ensemble",
      album: "City Mysteries",
      origin: "Scores",
      genre: "ambient",
      length_s: 180,
      bpm: null,
      match_score: 0.73,
      confidence: "medium",
      reasons: ["Genre metadata: ambient"],
      default_selected: true,
    },
  ],
};

const preview: AuthoringImportPreview = {
  source: {
    type: "document",
    id: "authoring-import/v1",
    name: "Assistant local playlist builder",
  },
  target_mode: { id: "dnd", name: "D&D" },
  items: [
    {
      kind: "playlist",
      resource_id: "0",
      name: "Rainy investigation",
      summary: "1 track · exploration",
      status: "ready",
      reason: null,
      issues: [],
    },
  ],
};

beforeEach(() => {
  vi.clearAllMocks();
  usePlayerStore.setState({
    state: { active_mode_id: "dnd" } as unknown as PlayerState,
  });
  vi.mocked(assistantApi.suggestPlaylist).mockResolvedValue(suggestion);
  vi.mocked(authoringImportApi.previewDocument).mockResolvedValue(preview);
  vi.mocked(authoringImportApi.commitDocument).mockResolvedValue({
    imported: [preview.items[0]],
    skipped: [],
    missing_track_paths: [],
  });
});

afterEach(() => {
  act(() => usePlayerStore.setState({ state: null }));
});

function renderView() {
  return render(
    <MemoryRouter>
      <PlaylistBuilderView />
    </MemoryRouter>,
  );
}

describe("PlaylistBuilderView", () => {
  it("explains local matches and creates only the reviewed selection", async () => {
    const user = userEvent.setup();
    renderView();

    await user.type(screen.getByLabelText("Mood or scene"), "dark rainy investigation");
    await user.click(screen.getByRole("button", { name: "Find matching songs" }));

    expect(await screen.findByText("Rainy Alley")).toBeInTheDocument();
    expect(screen.getByText("Distant Footsteps")).toBeInTheDocument();
    expect(screen.getByRole("meter", { name: "Tension" })).toHaveAttribute(
      "aria-valuenow",
      "86",
    );
    expect(screen.getByRole("checkbox", { name: "Include Rainy Alley" })).toBeChecked();

    await user.click(
      screen.getByRole("checkbox", { name: "Include Distant Footsteps" }),
    );
    await user.type(screen.getByLabelText("Playlist name"), "Rainy investigation");
    await user.type(screen.getByLabelText("Category"), "exploration");
    await user.click(screen.getByRole("button", { name: "Review playlist" }));

    const expectedDocument = {
      schema: "authoring-import/v1",
      name: "Local suggestion: dark rainy investigation",
      playlists: [
        {
          name: "Rainy investigation",
          category: "exploration",
          tracks: ["Scores/Rainy Alley.flac"],
        },
      ],
    };
    await waitFor(() =>
      expect(authoringImportApi.previewDocument).toHaveBeenCalledWith(
        "dnd",
        expectedDocument,
        "Assistant local playlist builder",
      ),
    );
    expect(await screen.findByText("Ready to create")).toBeInTheDocument();

    await user.click(screen.getByRole("button", { name: "Create playlist" }));
    await waitFor(() =>
      expect(authoringImportApi.commitDocument).toHaveBeenCalledWith(
        "dnd",
        expectedDocument,
        [{ kind: "playlist", resource_id: "0" }],
        "Assistant local playlist builder",
      ),
    );
    expect(toast.success).toHaveBeenCalledWith(
      "Playlist created",
      "Rainy investigation is ready in Authoring.",
    );
    expect(screen.getByRole("link", { name: "Open in Authoring" })).toHaveAttribute(
      "href",
      "/authoring/playlists",
    );
  });

  it("shows a recoverable error when local ranking fails", async () => {
    vi.mocked(assistantApi.suggestPlaylist).mockRejectedValueOnce(
      new Error("Library index is unavailable"),
    );
    const user = userEvent.setup();
    renderView();

    await user.type(screen.getByLabelText("Mood or scene"), "calm tavern");
    await user.click(screen.getByRole("button", { name: "Find matching songs" }));

    expect(await screen.findByText("Library index is unavailable")).toBeInTheDocument();
    expect(screen.getByRole("button", { name: "Find matching songs" })).toBeEnabled();
  });
});
