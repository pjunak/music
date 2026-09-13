import { render, screen } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { beforeEach, describe, expect, it, vi } from "vitest";

import type * as ApiModule from "@/core/api";
import type { Track } from "@/core/types";

vi.mock("@/core/toast", () => ({
  toast: { success: vi.fn(), warn: vi.fn(), error: vi.fn() },
}));
vi.mock("@/core/api", async (importActual) => {
  const actual = await importActual<typeof ApiModule>();
  return {
    ...actual,
    libraryApi: {
      ...actual.libraryApi,
      updateBulkMetadata: vi
        .fn()
        .mockResolvedValue({ updated: [{}], skipped: [] }),
    },
  };
});

import { libraryApi } from "@/core/api";

import { TagInspector } from "./TagInspector";

function track(p: Partial<Track>): Track {
  return {
    id: 1,
    path: "a.flac",
    title: "",
    artist: "",
    album_artist: "",
    album: "",
    track_no: null,
    year: null,
    genre: "",
    display_title: "",
    origin: "",
    length_s: 0,
    added_at: "",
    ...p,
  } as unknown as Track;
}

beforeEach(() => {
  vi.clearAllMocks();
});

describe("TagInspector", () => {
  it("preserves untouched full dates and credits and validates edits", async () => {
    render(<TagInspector selectedTracks={[track({ release_date: "2024-02-29", original_release_date: "1998-07", composer: "久石 譲" })]} onSaved={() => {}} />);
    const date = screen.getByLabelText("Release date");
    expect(date).toHaveValue("2024-02-29");
    expect(screen.getByLabelText("Original release date")).toHaveValue("1998-07");
    expect(screen.getByLabelText("Composer")).toHaveValue("久石 譲");
    await userEvent.type(screen.getByLabelText("Title"), "New title");
    await userEvent.click(screen.getByRole("button", { name: "Save" }));
    expect(libraryApi.updateBulkMetadata).toHaveBeenLastCalledWith({ track_ids: [1], updates: { title: "New title" } });
    await userEvent.clear(date);
    await userEvent.type(date, "2025-02-29");
    expect(screen.getByRole("alert")).toHaveTextContent("valid calendar date");
    expect(screen.getByRole("button", { name: "Save" })).toBeDisabled();
    await userEvent.clear(date);
    await userEvent.type(date, "2025-10");
    await userEvent.clear(screen.getByLabelText("Composer"));
    await userEvent.type(screen.getByLabelText("Composer"), "Composer A; Composer B");
    await userEvent.click(screen.getByRole("button", { name: "Save" }));
    expect(libraryApi.updateBulkMetadata).toHaveBeenLastCalledWith({ track_ids: [1], updates: { title: "New title", release_date: "2025-10", composer: "Composer A; Composer B" } });
  });

  it("shows an empty prompt when nothing is selected", () => {
    render(<TagInspector selectedTracks={[]} onSaved={() => {}} />);
    expect(screen.getByText(/select a track/i)).toBeInTheDocument();
  });

  it("edits one track and writes ONLY the changed field", async () => {
    const t = track({ id: 7, title: "Old", artist: "Keep" });
    render(<TagInspector selectedTracks={[t]} onSaved={() => {}} />);

    // Save is disabled until something changes.
    expect(screen.getByRole("button", { name: "Save" })).toBeDisabled();

    const title = screen.getByLabelText("Title");
    await userEvent.clear(title);
    await userEvent.type(title, "New");
    await userEvent.click(screen.getByRole("button", { name: "Save" }));

    expect(libraryApi.updateBulkMetadata).toHaveBeenCalledWith({
      track_ids: [7],
      updates: { title: "New" },
    });
  });

  it("shows ‹various› for fields that differ and writes a touched field to all", async () => {
    const a = track({ id: 1, artist: "A", album: "Shared" });
    const b = track({ id: 2, artist: "B", album: "Shared" });
    render(<TagInspector selectedTracks={[a, b]} onSaved={() => {}} />);

    const artist = screen.getByLabelText("Artist");
    expect(artist).toHaveValue(""); // differs → blank
    expect(artist).toHaveAttribute("placeholder", "‹various›");
    expect(screen.getByLabelText("Album")).toHaveValue("Shared"); // shared

    const genre = screen.getByLabelText("Genre");
    await userEvent.type(genre, "Rock");
    await userEvent.click(screen.getByRole("button", { name: /save to 2/i }));

    expect(libraryApi.updateBulkMetadata).toHaveBeenCalledWith({
      track_ids: [1, 2],
      updates: { genre: "Rock" },
    });
  });

  it("clears the release date including a legacy year value", async () => {
    const t = track({ id: 3, year: 1999 });
    render(<TagInspector selectedTracks={[t]} onSaved={() => {}} />);
    const year = screen.getByLabelText("Release date");
    expect(year).toHaveValue("1999");
    await userEvent.clear(year);
    await userEvent.click(screen.getByRole("button", { name: "Save" }));
    expect(libraryApi.updateBulkMetadata).toHaveBeenCalledWith({
      track_ids: [3],
      updates: { release_date: "" },
    });
  });
});
