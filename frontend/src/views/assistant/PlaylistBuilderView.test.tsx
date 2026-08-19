import { act, render, screen, waitFor } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { MemoryRouter } from "react-router-dom";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";

import type * as ApiModule from "@/core/api";
import type {
  AuthoringImportPreview,
  BackgroundJob,
  ModelPlaylistAvailability,
  PlaylistSuggestion,
} from "@/core/api";
import { usePlayerStore } from "@/core/playerStore";
import type { PlayerState } from "@/core/types";

vi.mock("@/core/api", async (importActual) => {
  const actual = await importActual<typeof ApiModule>();
  return {
    ...actual,
    assistantApi: {
      suggestPlaylist: vi.fn(),
      getModelPlaylistAvailability: vi.fn(),
      startModelPlaylistSuggestion: vi.fn(),
    },
    jobsApi: {
      list: vi.fn(),
      get: vi.fn(),
      cancel: vi.fn(),
      retry: vi.fn(),
    },
    authoringImportApi: {
      ...actual.authoringImportApi,
      previewDocument: vi.fn(),
      commitDocument: vi.fn(),
    },
  };
});

vi.mock("@/components/confirmDialog", () => ({ confirmDialog: vi.fn() }));

vi.mock("@/core/toast", () => ({
  toast: {
    success: vi.fn(),
    error: vi.fn(),
  },
}));

import { confirmDialog } from "@/components/confirmDialog";
import { assistantApi, authoringImportApi, jobsApi } from "@/core/api";
import { toast } from "@/core/toast";

import { PlaylistBuilderView } from "./PlaylistBuilderView";

const suggestion: PlaylistSuggestion = {
  engine: "local-planner/v2",
  library_tracks: 24,
  eligible_tracks: 22,
  intent: {
    matched_moods: ["tense", "dark"],
    search_terms: ["rainy"],
    energy: 0.48,
    brightness: 0.21,
    tension: 0.86,
  },
  plan: {
    energy_curve: "rising",
    selected_tracks: 2,
    selected_duration_s: 420,
    audio_profile_tracks: 1,
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
      manual_tags: ["investigation"],
      analysis_tags: ["tense", "dark"],
      length_s: 240,
      bpm: 92,
      match_score: 0.91,
      confidence: "high",
      reasons: ["Metadata matches: rainy", "92 BPM supports the requested pace"],
      default_selected: true,
      sequence_position: 1,
      planning_energy: 0.48,
      audio_signal: {
        analyzer_id: "local-audio/v1",
        energy: 0.44,
        brightness: 0.2,
        tension: 0.8,
        tempo_bpm: 91.8,
        confidence: "high",
      },
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
      manual_tags: [],
      analysis_tags: ["calm"],
      length_s: 180,
      bpm: null,
      match_score: 0.73,
      confidence: "medium",
      reasons: ["Genre metadata: ambient"],
      default_selected: true,
      sequence_position: 2,
      planning_energy: 0.61,
      audio_signal: null,
    },
  ],
};

const modelSuggestion: PlaylistSuggestion = {
  ...suggestion,
  engine: "model-playlist-planner/v1",
};

const modelAvailability: ModelPlaylistAvailability = {
  available: true,
  reason_code: null,
  role_id: "playlist_planner",
  connection_name: "My models",
  model_id: "planner-large",
  quality_evaluation_id: "playlist-quality-v1",
  job_kind: "assistant.model-playlist-suggestion",
  disclosure: {
    version: "assistant-playlist-model-disclosure/v1",
    shared_with_provider: [
      "Your mood prompt and filters",
      "Up to 100 locally prefiltered candidate IDs and metadata",
    ],
    never_shared: ["Audio files or cover artwork", "Filesystem paths"],
    maximum_candidates: 100,
    may_incur_cost: true,
  },
};

function modelJob(
  status: BackgroundJob["status"],
  overrides: Partial<BackgroundJob> = {},
): BackgroundJob {
  return {
    id: "model-job-1",
    kind: "assistant.model-playlist-suggestion",
    status,
    parameters: {
      consent: true,
      disclosure_version: "assistant-playlist-model-disclosure/v1",
      request: {
        prompt: "misty medieval forest",
        target_minutes: 45,
        candidate_limit: 40,
        energy_curve: "arc",
        include_unknown_bpm: true,
      },
    },
    result:
      status === "succeeded"
        ? {
            schema_version: "assistant-playlist-suggestion-job-result/v1",
            disclosure_version: "assistant-playlist-model-disclosure/v1",
            role_id: "playlist_planner",
            role_fingerprint: "a".repeat(64),
            suggestion: modelSuggestion,
          }
        : null,
    error: status === "failed" ? "Provider timed out" : null,
    progress_current: status === "running" ? 2 : status === "succeeded" ? 3 : 0,
    progress_total: 3,
    progress_phase: status === "running" ? "Waiting for playlist model" : "Complete",
    progress_message:
      status === "running"
        ? "Sending the disclosed, path-free candidate pool"
        : "",
    attempts: 1,
    retry_of_id: null,
    created_at: "2026-08-19T10:00:00Z",
    updated_at: "2026-08-19T10:00:00Z",
    started_at: status === "queued" ? null : "2026-08-19T10:00:00Z",
    finished_at:
      status === "succeeded" || status === "failed" || status === "cancelled"
        ? "2026-08-19T10:01:00Z"
        : null,
    ...overrides,
  };
}

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
  vi.mocked(assistantApi.getModelPlaylistAvailability).mockResolvedValue({
    ...modelAvailability,
    available: false,
    reason_code: "model_quality_not_passed",
  });
  vi.mocked(assistantApi.startModelPlaylistSuggestion).mockResolvedValue(
    modelJob("running"),
  );
  vi.mocked(jobsApi.list).mockResolvedValue([]);
  vi.mocked(jobsApi.get).mockResolvedValue(modelJob("succeeded"));
  vi.mocked(jobsApi.cancel).mockResolvedValue(modelJob("cancelled"));
  vi.mocked(confirmDialog).mockResolvedValue(true);
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
    await user.selectOptions(screen.getByLabelText("Playlist flow"), "rising");
    await user.click(screen.getByRole("button", { name: "Find matching songs" }));

    expect(await screen.findByText("Rainy Alley")).toBeInTheDocument();
    expect(assistantApi.suggestPlaylist).toHaveBeenCalledWith(
      expect.objectContaining({ energy_curve: "rising" }),
    );
    expect(screen.getByText("Your tags")).toBeInTheDocument();
    expect(screen.getAllByText("Analysis")).toHaveLength(2);
    expect(screen.getByText("Audio signal")).toBeInTheDocument();
    expect(screen.getByText("≈92 BPM")).toBeInTheDocument();
    expect(screen.getByText(/Rising intensity/)).toBeInTheDocument();
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

  it("confirms the disclosure, persists progress, and restores the model draft", async () => {
    vi.mocked(assistantApi.getModelPlaylistAvailability).mockResolvedValue(
      modelAvailability,
    );
    const user = userEvent.setup();
    renderView();

    await user.click(
      await screen.findByRole("radio", { name: /Connected model/ }),
    );
    expect(screen.getByText("Review what leaves the server")).toBeInTheDocument();
    expect(screen.getByText("Filesystem paths")).toBeInTheDocument();
    await user.type(screen.getByLabelText("Mood or scene"), "misty medieval forest");
    await user.click(
      screen.getByRole("button", { name: "Review disclosure and start" }),
    );

    expect(confirmDialog).toHaveBeenCalledWith(
      expect.objectContaining({
        confirmLabel: "Send candidates and start",
        body: expect.stringContaining("No audio or file paths are sent"),
      }),
    );
    expect(assistantApi.startModelPlaylistSuggestion).toHaveBeenCalledWith(
      expect.objectContaining({ prompt: "misty medieval forest" }),
      "assistant-playlist-model-disclosure/v1",
    );
    expect(
      await screen.findByRole("progressbar", {
        name: "Connected model suggestion progress",
      }),
    ).toBeInTheDocument();
    expect(screen.getByText("Waiting for playlist model")).toBeInTheDocument();

    expect(
      await screen.findByText("Ranked with connected model", {}, { timeout: 2500 }),
    ).toBeInTheDocument();
    expect(screen.getByText("Rainy Alley")).toBeInTheDocument();
    expect(assistantApi.suggestPlaylist).not.toHaveBeenCalled();

    await user.type(screen.getByLabelText("Playlist name"), "Misty forest");
    await user.click(screen.getByRole("button", { name: "Review playlist" }));
    await waitFor(() =>
      expect(authoringImportApi.previewDocument).toHaveBeenCalledWith(
        "dnd",
        {
          schema: "authoring-import/v1",
          name: "Model suggestion: misty medieval forest",
          playlists: [
            {
              name: "Misty forest",
              category: null,
              tracks: [
                "Scores/Rainy Alley.flac",
                "Scores/Distant Footsteps.flac",
              ],
            },
          ],
        },
        "Assistant model playlist builder",
      ),
    );
  });

  it("restores a running job after reopen, cancels it, and offers local fallback", async () => {
    vi.mocked(assistantApi.getModelPlaylistAvailability).mockResolvedValue(
      modelAvailability,
    );
    vi.mocked(jobsApi.list).mockResolvedValue([modelJob("running")]);
    const user = userEvent.setup();
    renderView();

    expect(await screen.findByText("Waiting for playlist model")).toBeInTheDocument();
    expect(screen.getByLabelText("Mood or scene")).toHaveValue(
      "misty medieval forest",
    );
    await user.click(
      screen.getByRole("button", { name: "Cancel model suggestion" }),
    );

    expect(jobsApi.cancel).toHaveBeenCalledWith("model-job-1");
    expect(
      await screen.findByText("The connected-model suggestion was cancelled."),
    ).toBeInTheDocument();
    await user.click(
      screen.getByRole("button", {
        name: "Use local planner with these settings",
      }),
    );

    expect(assistantApi.suggestPlaylist).toHaveBeenCalledWith(
      expect.objectContaining({
        prompt: "misty medieval forest",
        target_minutes: 45,
        energy_curve: "arc",
      }),
    );
    expect(await screen.findByText("Ranked locally")).toBeInTheDocument();
  });

  it("restores a completed model draft without repeating provider work", async () => {
    vi.mocked(assistantApi.getModelPlaylistAvailability).mockResolvedValue(
      modelAvailability,
    );
    vi.mocked(jobsApi.list).mockResolvedValue([modelJob("succeeded")]);
    renderView();

    expect(
      await screen.findByText("Ranked with connected model"),
    ).toBeInTheDocument();
    expect(screen.getByLabelText("Mood or scene")).toHaveValue(
      "misty medieval forest",
    );
    expect(assistantApi.startModelPlaylistSuggestion).not.toHaveBeenCalled();
    expect(jobsApi.get).not.toHaveBeenCalled();
  });
});
