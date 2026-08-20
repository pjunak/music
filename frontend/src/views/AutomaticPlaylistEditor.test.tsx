import { render, screen, waitFor } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { beforeEach, describe, expect, it, vi } from "vitest";

import type * as ApiModule from "@/core/api";
import type { AutomaticPlaylistPreview, PlaylistMeta } from "@/core/types";

vi.mock("@/core/api", async (importActual) => {
  const actual = await importActual<typeof ApiModule>();
  return {
    ...actual,
    playlistsApi: {
      ...actual.playlistsApi,
      previewAutomatic: vi.fn(),
      configureAutomatic: vi.fn(),
      refreshAutomatic: vi.fn(),
      disableAutomatic: vi.fn(),
    },
  };
});

vi.mock("@/components/confirmDialog", () => ({ confirmDialog: vi.fn() }));
vi.mock("@/core/toast", () => ({
  toast: { success: vi.fn(), error: vi.fn() },
}));

import { confirmDialog } from "@/components/confirmDialog";
import { playlistsApi } from "@/core/api";

import { AutomaticPlaylistEditor } from "./AutomaticPlaylistEditor";

const manualPlaylist: PlaylistMeta = {
  id: 7,
  name: "Living Tavern",
  mode_id: "dnd",
  category: null,
  automatic: false,
  automatic_rule: null,
  automatic_rule_error: null,
  automatic_refreshed_at: null,
  created_at: "2026-08-19T10:00:00Z",
  updated_at: "2026-08-19T10:00:00Z",
};

const preview: AutomaticPlaylistPreview = {
  schema_version: "automatic-playlist-preview/v1",
  source_signature: "a".repeat(64),
  library_tracks: 20,
  matched_tracks: 2,
  tracks: [
    {
      id: 1,
      path: "Tavern/Dance.flac",
      title: "Tavern Dance",
      artist: "The Minstrels",
      album: "Inn Songs",
      length_s: 180,
      bpm: 118,
    },
    {
      id: 2,
      path: "Tavern/Rest.flac",
      title: "Quiet Inn",
      artist: "The Minstrels",
      album: "Inn Songs",
      length_s: 200,
      bpm: 82,
    },
  ],
};

describe("AutomaticPlaylistEditor", () => {
  beforeEach(() => {
    vi.clearAllMocks();
    vi.mocked(playlistsApi.previewAutomatic).mockResolvedValue(preview);
    vi.mocked(playlistsApi.configureAutomatic).mockResolvedValue({
      schema_version: "automatic-playlist-apply/v1",
      playlist: {
        ...manualPlaylist,
        automatic: true,
        automatic_rule: {
          schema: "automatic-playlist/v1",
          include_tags: ["tavern", "dancing"],
          match: "any",
          exclude_tags: [],
          tag_sources: "manual",
          min_bpm: null,
          max_bpm: null,
          include_unknown_bpm: true,
          maximum_tracks: 200,
          order_by: "title",
        },
        automatic_refreshed_at: "2026-08-19T10:01:00Z",
      },
      materialized_tracks: 2,
    });
  });

  it("requires preview before explicitly enabling the local rule", async () => {
    const user = userEvent.setup();
    const onChanged = vi.fn().mockResolvedValue(undefined);
    const onTracksChanged = vi.fn().mockResolvedValue(undefined);
    render(
      <AutomaticPlaylistEditor
        playlist={manualPlaylist}
        onChanged={onChanged}
        onTracksChanged={onTracksChanged}
      />,
    );

    await user.click(screen.getByRole("button", { name: "Set up automatic rule" }));
    await user.type(
      screen.getByLabelText("Include tags (comma separated)"),
      "tavern, dancing",
    );
    expect(
      screen.queryByRole("button", { name: "Enable automatic playlist" }),
    ).not.toBeInTheDocument();
    await user.click(screen.getByRole("button", { name: "Preview matching songs" }));

    expect(await screen.findByText("Tavern Dance")).toBeInTheDocument();
    expect(screen.getByText("Quiet Inn")).toBeInTheDocument();
    expect(playlistsApi.previewAutomatic).toHaveBeenCalledWith(
      7,
      expect.objectContaining({
        schema: "automatic-playlist/v1",
        include_tags: ["tavern", "dancing"],
        tag_sources: "manual",
      }),
    );
    await user.click(
      screen.getByRole("button", { name: "Enable automatic playlist" }),
    );

    await waitFor(() => expect(playlistsApi.configureAutomatic).toHaveBeenCalled());
    expect(playlistsApi.configureAutomatic).toHaveBeenCalledWith(
      7,
      expect.objectContaining({ include_tags: ["tavern", "dancing"] }),
      "a".repeat(64),
    );
    expect(onChanged).toHaveBeenCalled();
    expect(onTracksChanged).toHaveBeenCalled();
  });

  it("keeps resolved tracks when the operator makes an automatic playlist manual", async () => {
    const user = userEvent.setup();
    vi.mocked(confirmDialog).mockResolvedValue(true);
    vi.mocked(playlistsApi.disableAutomatic).mockResolvedValue(manualPlaylist);
    const automatic: PlaylistMeta = {
      ...manualPlaylist,
      automatic: true,
      automatic_rule: {
        schema: "automatic-playlist/v1",
        include_tags: ["tavern"],
        match: "any",
        exclude_tags: ["combat"],
        tag_sources: "manual",
        min_bpm: null,
        max_bpm: null,
        include_unknown_bpm: true,
        maximum_tracks: 200,
        order_by: "title",
      },
      automatic_refreshed_at: "2026-08-19T10:01:00Z",
    };
    const onChanged = vi.fn().mockResolvedValue(undefined);
    const onTracksChanged = vi.fn().mockResolvedValue(undefined);
    render(
      <AutomaticPlaylistEditor
        playlist={automatic}
        onChanged={onChanged}
        onTracksChanged={onTracksChanged}
      />,
    );

    expect(screen.getByText("Local rule is active")).toBeInTheDocument();
    await user.click(screen.getByRole("button", { name: "Make manual" }));

    await waitFor(() => expect(playlistsApi.disableAutomatic).toHaveBeenCalledWith(7));
    expect(confirmDialog).toHaveBeenCalledWith(
      expect.objectContaining({ confirmLabel: "Make manual" }),
    );
    expect(onChanged).toHaveBeenCalled();
    expect(onTracksChanged).toHaveBeenCalled();
  });

  it("offers recovery when a saved automatic rule is unreadable", async () => {
    const user = userEvent.setup();
    vi.mocked(confirmDialog).mockResolvedValue(true);
    vi.mocked(playlistsApi.disableAutomatic).mockResolvedValue(manualPlaylist);
    const damaged: PlaylistMeta = {
      ...manualPlaylist,
      automatic: true,
      automatic_rule: null,
      automatic_rule_error: "automatic_rule_invalid",
    };
    const onChanged = vi.fn().mockResolvedValue(undefined);
    const onTracksChanged = vi.fn().mockResolvedValue(undefined);
    render(
      <AutomaticPlaylistEditor
        playlist={damaged}
        onChanged={onChanged}
        onTracksChanged={onTracksChanged}
      />,
    );

    expect(screen.getByText("The saved rule cannot be read")).toBeInTheDocument();
    expect(
      screen.getByText(/last resolved songs are being kept/i),
    ).toBeInTheDocument();
    await user.click(screen.getByRole("button", { name: "Make manual" }));

    await waitFor(() => expect(playlistsApi.disableAutomatic).toHaveBeenCalledWith(7));
    expect(onChanged).toHaveBeenCalled();
    expect(onTracksChanged).toHaveBeenCalled();
  });
});
