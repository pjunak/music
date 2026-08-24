import { render, screen } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";

import type * as ApiModule from "@/core/api";
import type { PlayerState, PlaylistMeta } from "@/core/types";

vi.mock("@/core/ws", () => ({ wsClient: { send: vi.fn() } }));
vi.mock("@/core/api", async (importActual) => {
  const actual = await importActual<typeof ApiModule>();
  return {
    ...actual,
    modesApi: { ...actual.modesApi, list: vi.fn() },
    libraryApi: {
      ...actual.libraryApi,
      allFolders: vi.fn(),
      tree: vi.fn(),
    },
    playlistsApi: {
      ...actual.playlistsApi,
      list: vi.fn(),
      tracks: vi.fn(),
    },
  };
});
vi.mock("./assistant/PlaylistBuilderView", () => ({
  PlaylistBuilderView: ({
    embedded,
    onCreated,
  }: {
    embedded?: boolean;
    onCreated?: (name: string) => void | Promise<void>;
  }) => (
    <button type="button" onClick={() => void onCreated?.("Assisted journey")}>
      {embedded ? "Finish assisted playlist" : "Standalone assistant"}
    </button>
  ),
}));

import { libraryApi, modesApi, playlistsApi } from "@/core/api";
import { usePlayerStore } from "@/core/playerStore";

import { PlaylistsView } from "./PlaylistsView";

const assisted: PlaylistMeta = {
  id: 42,
  name: "Assisted journey",
  mode_id: "dnd",
  category: "travel",
  automatic: false,
  automatic_rule: null,
  automatic_rule_error: null,
  automatic_refreshed_at: null,
  created_at: "2026-08-24T10:00:00Z",
  updated_at: "2026-08-24T10:00:00Z",
};

beforeEach(() => {
  vi.clearAllMocks();
  vi.mocked(modesApi.list).mockResolvedValue([]);
  vi.mocked(libraryApi.allFolders).mockResolvedValue({ folders: [] });
  vi.mocked(libraryApi.tree).mockResolvedValue({ path: "", tracks: [] });
  vi.mocked(playlistsApi.list)
    .mockResolvedValueOnce([])
    .mockResolvedValue([assisted]);
  vi.mocked(playlistsApi.tracks).mockResolvedValue([]);
  usePlayerStore.setState({
    state: {
      active_mode_id: "dnd",
      ambient: { current_track_id: null },
      interrupt: null,
    } as unknown as PlayerState,
  });
});

afterEach(() => {
  usePlayerStore.setState({ state: null });
});

describe("Playlist Authoring assistance", () => {
  it("keeps the Assistant in Authoring and opens the created playlist for editing", async () => {
    const user = userEvent.setup();
    render(<PlaylistsView />);

    await user.click(await screen.findByRole("button", { name: "Assist" }));
    expect(screen.getByRole("heading", { name: "Draft a playlist" })).toBeVisible();
    await user.click(screen.getByRole("button", { name: "Finish assisted playlist" }));

    expect(await screen.findByRole("heading", { name: "Assisted journey" })).toBeVisible();
    expect(screen.getByLabelText("Category")).toHaveValue("travel");
  });
});
