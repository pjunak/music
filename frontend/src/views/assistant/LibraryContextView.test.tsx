import { fireEvent, render, screen, waitFor } from "@testing-library/react";
import { MemoryRouter } from "react-router-dom";
import { beforeEach, describe, expect, it, vi } from "vitest";

import type * as ApiModule from "@/core/api";
import type { TrackContextDetail } from "@/core/api";
import type { Track } from "@/core/types";

vi.mock("@/core/api", async (importActual) => {
  const actual = await importActual<typeof ApiModule>();
  return {
    ...actual,
    assistantApi: { ...actual.assistantApi, getTrackContext: vi.fn() },
    libraryApi: {
      ...actual.libraryApi,
      allFolders: vi.fn(),
      tree: vi.fn(),
      streamUrl: (id: number) => `/api/library/tracks/${id}/stream`,
    },
  };
});

import { assistantApi, libraryApi } from "@/core/api";

import { LibraryContextView } from "./LibraryContextView";

const track: Track = {
  id: 9,
  path: "Campaign/Forest/Quiet Road.flac",
  title: "Quiet Road",
  display_title: "",
  artist: "Tabletop Ensemble",
  album_artist: "",
  album: "Wilderness",
  track_no: null,
  disc_no: null,
  year: null,
  genre: "ambient",
  length_s: 180,
  bpm: 72,
  size_bytes: 1000,
  added_at: "2026-08-24T10:00:00Z",
  origin: "",
};

const secondTrack: Track = {
  ...track,
  id: 10,
  path: "Campaign/Forest/Second Watch.flac",
  title: "Second Watch",
};

const trajectory = {
  typical: 0.5,
  low: 0.2,
  high: 0.8,
  range: 0.6,
  variability: 0.3,
  slope: 0.4,
  start: 0.3,
  end: 0.7,
  peak_at_fraction: 0.8,
  high_fraction: 0.2,
  shape: "gradual_rise",
};

const detail: TrackContextDetail = {
  track_id: 9,
  title: "Quiet Road",
  artist: "Tabletop Ensemble",
  status: "full",
  analyzer_id: "local-context/v2",
  confidence: "high",
  updated_at: "2026-08-24T10:00:00Z",
  summary: {
    trajectories: {
      loudness: trajectory,
      intensity: trajectory,
      rhythmic_drive: trajectory,
      brightness: trajectory,
      density: trajectory,
      spectral_flux: trajectory,
    },
    tempo: {
      status: "measured",
      typical_bpm: 72,
      low_bpm: 70,
      high_bpm: 74,
    },
    structure: {
      section_count: 1,
      development: "continuous",
    },
    voice: {
      status: "not_classified",
      note: "Local voice classification is not enabled.",
    },
  },
  timeline: [
    { start_s: 0, duration_s: 2, intensity: 0.2, rhythmic_drive: 0.3, loudness: 0.2 },
    { start_s: 178, duration_s: 2, intensity: 0.8, rhythmic_drive: 0.7, loudness: 0.75 },
  ],
  sections: [
    {
      id: "s1",
      start_s: 0,
      end_s: 180,
      intensity: 0.5,
      rhythmic_drive: 0.5,
      brightness: 0.5,
      density: 0.5,
      changes_from_previous: [],
    },
  ],
  technical: { codec: "flac" },
  stages: { voice: { status: "not_configured" } },
  error: null,
};

beforeEach(() => {
  vi.clearAllMocks();
  vi.mocked(libraryApi.allFolders).mockResolvedValue({
    folders: [
      { name: "Campaign", path: "Campaign", track_count: 0, has_children: true },
      {
        name: "Forest",
        path: "Campaign/Forest",
        track_count: 1,
        has_children: false,
      },
    ],
  });
  vi.mocked(libraryApi.tree).mockResolvedValue({ path: "", tracks: [track] });
  vi.mocked(assistantApi.getTrackContext).mockResolvedValue(detail);
});

describe("LibraryContextView", () => {
  it("mirrors the library and shows time-aware context with playback", async () => {
    const { container } = render(
      <MemoryRouter>
        <LibraryContextView />
      </MemoryRouter>,
    );

    expect(await screen.findByRole("heading", { name: "Quiet Road" })).toBeInTheDocument();
    expect(screen.queryByText("Development across the track")).not.toBeInTheDocument();
    expect(screen.getAllByText("gradual rise · 20%–80%")).toHaveLength(6);
    const graph = screen.getByLabelText(/Intensity, rhythmic drive/);
    expect(graph).toHaveAttribute("preserveAspectRatio", "none");
    const player = container.querySelector("audio");
    expect(player).not.toBeNull();
    if (player === null) throw new Error("expected the track preview audio element");
    expect(player).toHaveAttribute("src", "/api/library/tracks/9/stream");
    expect(player).not.toHaveAttribute("controls");
    const heading = screen.getByRole("heading", { name: "Quiet Road" });
    expect(heading.parentElement).toHaveTextContent("Quiet RoadTabletop Ensemble");
    expect(
      container.querySelector(".assistant-context-detail-heading > .assistant-context-status"),
    ).toHaveTextContent("full · high confidence");
    expect(screen.getByText("0:00 / 3:00")).toBeInTheDocument();
    Object.defineProperty(player, "currentTime", { configurable: true, writable: true, value: 90 });
    fireEvent.timeUpdate(player);
    expect(await screen.findByText("1:30 / 3:00")).toBeInTheDocument();
    expect(screen.queryByText("Campaign/Forest/Quiet Road.flac")).not.toBeInTheDocument();
    await waitFor(() => expect(assistantApi.getTrackContext).toHaveBeenCalledWith(9));
  });

  it("uses the graph scrubber and custom preview controls", async () => {
    const { container } = render(
      <MemoryRouter>
        <LibraryContextView />
      </MemoryRouter>,
    );

    expect(await screen.findByRole("heading", { name: "Quiet Road" })).toBeInTheDocument();
    const player = container.querySelector("audio");
    if (player === null) throw new Error("expected the track preview audio element");
    Object.defineProperty(player, "currentTime", { configurable: true, writable: true, value: 0 });
    const play = vi.fn().mockResolvedValue(undefined);
    const pause = vi.fn();
    Object.defineProperty(player, "play", { configurable: true, value: play });
    Object.defineProperty(player, "pause", { configurable: true, value: pause });

    const scrubber = screen.getByRole("slider", { name: "Seek Quiet Road" });
    fireEvent.change(scrubber, { target: { value: "45" } });
    expect(player.currentTime).toBe(45);
    expect(screen.getByText("0:45 / 3:00")).toBeInTheDocument();

    fireEvent.click(screen.getByRole("button", { name: "Play Quiet Road" }));
    expect(play).toHaveBeenCalledOnce();
    fireEvent.play(player);
    expect(screen.getByRole("button", { name: "Pause Quiet Road" })).toBeInTheDocument();

    fireEvent.change(screen.getByRole("slider", { name: "Quiet Road preview volume" }), {
      target: { value: "0.35" },
    });
    expect(player.volume).toBe(0.35);

    fireEvent.click(screen.getByRole("button", { name: "Stop Quiet Road" }));
    expect(pause).toHaveBeenCalledOnce();
    expect(player.currentTime).toBe(0);
    expect(screen.getByText("0:00 / 3:00")).toBeInTheDocument();
  });

  it("keeps the playback panel compact and resets detail scrolling for a new track", async () => {
    vi.mocked(libraryApi.tree).mockResolvedValue({ path: "", tracks: [track, secondTrack] });
    vi.mocked(assistantApi.getTrackContext).mockImplementation(async (trackId) =>
      trackId === secondTrack.id
        ? { ...detail, track_id: secondTrack.id, title: secondTrack.title }
        : detail,
    );
    const { container } = render(
      <MemoryRouter>
        <LibraryContextView />
      </MemoryRouter>,
    );

    expect(await screen.findByRole("heading", { name: "Quiet Road" })).toBeInTheDocument();
    const playbackPanel = container.querySelector(".assistant-context-development");
    const trajectorySummary = screen.getByRole("region", { name: "Track development summary" });
    expect(playbackPanel).not.toContainElement(trajectorySummary);
    expect(playbackPanel?.nextElementSibling).toBe(trajectorySummary);

    const firstScroller = container.querySelector<HTMLElement>(".assistant-context-detail");
    if (firstScroller === null) throw new Error("expected the track detail scroller");
    firstScroller.scrollTop = 480;
    fireEvent.click(screen.getByText("Second Watch"));

    expect(await screen.findByRole("heading", { name: "Second Watch" })).toBeInTheDocument();
    await waitFor(() => {
      expect(container.querySelector<HTMLElement>(".assistant-context-detail")?.scrollTop).toBe(0);
    });
  });

  it("shows bounded classifier score and coverage when voice analysis is configured", async () => {
    vi.mocked(assistantApi.getTrackContext).mockResolvedValue({
      ...detail,
      summary: {
        ...detail.summary,
        voice: {
          status: "classified",
          voice_probability: 0.82,
          vocal_coverage: 0.75,
          note: "Voice is present across most analyzed windows.",
        },
      },
    });

    render(
      <MemoryRouter>
        <LibraryContextView />
      </MemoryRouter>,
    );

    expect(await screen.findByText("Voice present")).toBeInTheDocument();
    expect(screen.getByText(/82% voice score · 75% vocal coverage/)).toBeInTheDocument();
  });
});
