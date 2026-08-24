import { render, screen, waitFor } from "@testing-library/react";
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
  analyzer_id: "local-context/v1",
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
      note: "No calibrated voice classifier is configured.",
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
    render(
      <MemoryRouter>
        <LibraryContextView />
      </MemoryRouter>,
    );

    expect(await screen.findByRole("heading", { name: "Quiet Road" })).toBeInTheDocument();
    expect(screen.getByText("Development across the track")).toBeInTheDocument();
    expect(screen.getAllByText("gradual rise · 20%–80%")).toHaveLength(6);
    expect(screen.getByLabelText(/Intensity, rhythmic drive/)).toBeInTheDocument();
    const player = screen.getByLabelText("Play Quiet Road");
    expect(player).toHaveAttribute("src", "/api/library/tracks/9/stream");
    expect(screen.queryByText("Campaign/Forest/Quiet Road.flac")).not.toBeInTheDocument();
    await waitFor(() => expect(assistantApi.getTrackContext).toHaveBeenCalledWith(9));
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
