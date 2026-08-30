import { render, screen, waitFor, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { MemoryRouter } from "react-router-dom";
import { beforeEach, describe, expect, it, vi } from "vitest";

import type * as ApiModule from "@/core/api";
import type {
  BackgroundJob,
  LibraryContextSummary,
  ModelTaggingAvailability,
} from "@/core/api";

vi.mock("@/core/api", async (importActual) => {
  const actual = await importActual<typeof ApiModule>();
  return {
    ...actual,
    assistantApi: {
      ...actual.assistantApi,
      startLibraryContextAnalysis: vi.fn(),
      getLibraryContextSummary: vi.fn(),
      planModelTagging: vi.fn(),
      startModelTagging: vi.fn(),
      getManualTagCatalog: vi.fn(),
      listLibraryTags: vi.fn(),
      getModelTagCleanupAvailability: vi.fn(),
    },
    jobsApi: {
      list: vi.fn(),
      get: vi.fn(),
      cancel: vi.fn(),
      retry: vi.fn(),
    },
  };
});

vi.mock("@/core/toast", () => ({
  toast: { success: vi.fn(), error: vi.fn() },
}));

import { assistantApi, jobsApi } from "@/core/api";
import { toast } from "@/core/toast";

import { LibraryAnalysisView } from "./LibraryAnalysisView";

const summary: LibraryContextSummary = {
  analyzer: "local-context/v2",
  voice_analyzer: {
    analyzer_id: "essentia-musicnn-voice/v1",
    status: "unavailable",
    reason: "model_missing",
    model_filename: "voice_instrumental-musicnn-msd-2.pb",
    model_sha256: "b734bca3fc99257cf0088211b44bd36e8a26fbb1f9ce67e1e97d39f188094b0a",
  },
  passes: {
    audio_context: {
      completed_tracks: 83,
      failed_tracks: 2,
      skipped_tracks: 0,
      total_tracks: 120,
      enabled: true,
    },
    voice_detection: {
      completed_tracks: 0,
      failed_tracks: 0,
      skipped_tracks: 120,
      total_tracks: 120,
      enabled: false,
    },
  },
  library_tracks: 120,
  analyzed_tracks: 83,
  full_tracks: 80,
  partial_tracks: 3,
  missing_tracks: 34,
  failed_tracks: 2,
  stale_tracks: 1,
  high_confidence: 35,
  medium_confidence: 33,
  low_confidence: 15,
  last_updated_at: "2026-08-24T10:00:00Z",
};

const unavailableTagger: ModelTaggingAvailability = {
  available: false,
  reason_code: "role_not_configured",
  role_id: "music_tagger",
  connection_name: null,
  model_id: null,
  quality_evaluation_id: "music-tagging-quality-v1",
  job_kind: "assistant.model-music-tagging",
  library_tracks: 120,
  scope_tracks: 120,
  planned_tracks: 120,
  tracks_with_full_context: 80,
  tracks_with_partial_context: 3,
  tracks_missing_context: 37,
  current_profiles: 0,
  tracks_needing_tags: 120,
  estimated_provider_requests: 6,
  disclosure: {
    version: "assistant-model-music-tagging-disclosure/v11",
    shared_with_provider: [],
    never_shared: [],
    allowed_tags: ["calm"],
    tracks_per_request: 20,
    invalid_response_retry_limit: 2,
    may_incur_cost: true,
  },
};

function job(overrides: Partial<BackgroundJob> = {}): BackgroundJob {
  return {
    id: "context-job-1",
    kind: "assistant.library-context-analysis",
    status: "running",
    parameters: { force: false, scope: { type: "all" } },
    result: null,
    error: null,
    progress_current: 42,
    progress_total: 120,
    progress_phase: "Building track context",
    progress_message: "Processed 42 of 120 tracks",
    attempts: 1,
    retry_of_id: null,
    created_at: "2026-08-24T10:00:00Z",
    updated_at: "2026-08-24T10:01:00Z",
    started_at: "2026-08-24T10:00:01Z",
    finished_at: null,
    ...overrides,
  };
}

function renderView() {
  return render(
    <MemoryRouter>
      <LibraryAnalysisView />
    </MemoryRouter>,
  );
}

beforeEach(() => {
  vi.clearAllMocks();
  vi.mocked(assistantApi.getLibraryContextSummary).mockResolvedValue(summary);
  vi.mocked(assistantApi.planModelTagging).mockResolvedValue(unavailableTagger);
  vi.mocked(assistantApi.getManualTagCatalog).mockResolvedValue({
    starter_groups: [],
    used_tags: [],
    tag_usage: [],
  });
  vi.mocked(assistantApi.listLibraryTags).mockResolvedValue({
    items: [],
    total: 0,
    offset: 0,
    limit: 50,
  });
  vi.mocked(assistantApi.getModelTagCleanupAvailability).mockResolvedValue({
    available: false,
    reason_code: "role_not_configured",
    role_id: "tag_cleanup",
    connection_name: null,
    model_id: null,
    quality_evaluation_id: "tag-cleanup-quality-v1",
    job_kind: "assistant.model-tag-cleanup",
    catalog_signature: "0".repeat(64),
    vocabulary_fingerprint: "1".repeat(64),
    manual_tags: 0,
    estimated_provider_requests: 0,
    disclosure: {
      version: "assistant-model-tag-cleanup-disclosure/v3",
      shared_with_provider: [],
      never_shared: [],
      maximum_tags: 500,
      may_incur_cost: true,
    },
  });
  vi.mocked(jobsApi.list).mockResolvedValue([]);
});

describe("LibraryAnalysisView", () => {
  it("shows the consolidated context coverage and restores durable progress", async () => {
    const running = job();
    vi.mocked(jobsApi.list).mockImplementation(async (params) =>
      params?.kind === "assistant.library-context-analysis" ? [running] : [],
    );
    vi.mocked(jobsApi.cancel).mockResolvedValue({
      ...running,
      status: "cancel_requested",
      progress_phase: "Cancelling",
    });
    const user = userEvent.setup();
    renderView();

    expect(await screen.findByText("Processed 42 of 120 tracks")).toBeInTheDocument();
    expect(screen.getByRole("progressbar", { name: "Audio context pass progress" })).toHaveValue(
      85,
    );
    expect(screen.getByRole("progressbar", { name: "Voice detection pass progress" })).toHaveValue(
      0,
    );
    expect(screen.getByText("Pass 1")).toBeInTheDocument();
    expect(screen.getByText("Pass 2")).toBeInTheDocument();
    expect(screen.getByText("Full")).toBeInTheDocument();
    expect(screen.getByText("Partial")).toBeInTheDocument();
    expect(
      screen.getByText(/voice_instrumental-musicnn-msd-2\.pb is missing/),
    ).toBeInTheDocument();
    expect(
      screen.getByRole("heading", { name: "Analysis in progress" }),
    ).toBeInTheDocument();
    expect(
      screen.queryByRole("heading", { name: "Evidence ready for tagging" }),
    ).not.toBeInTheDocument();
    await user.click(screen.getByRole("button", { name: "Cancel" }));
    expect(jobsApi.cancel).toHaveBeenCalledWith("context-job-1");
  });

  it("shows independently checkpointed progress for the two analysis passes", async () => {
    vi.mocked(assistantApi.getLibraryContextSummary).mockResolvedValue({
      ...summary,
      voice_analyzer: { ...summary.voice_analyzer, status: "ready", reason: null },
      passes: {
        audio_context: {
          completed_tracks: 120,
          failed_tracks: 0,
          skipped_tracks: 0,
          total_tracks: 120,
          enabled: true,
        },
        voice_detection: {
          completed_tracks: 14,
          failed_tracks: 0,
          skipped_tracks: 0,
          total_tracks: 120,
          enabled: true,
        },
      },
    });
    vi.mocked(jobsApi.list).mockImplementation(async (params) =>
      params?.kind === "assistant.library-context-analysis"
        ? [
            job({
              status: "failed",
              error: "BrokenProcessPool: a voice worker terminated abruptly",
              progress_phase: "Detecting voice",
              progress_message: "Voice detection: 14 of 120 eligible tracks processed",
              result: {
                schema_version: "assistant-library-context-job-progress/v1",
                passes: {
                  audio_context: {
                    status: "complete",
                    completed_tracks: 120,
                    failed_tracks: 0,
                    skipped_tracks: 0,
                    total_tracks: 120,
                  },
                  voice_detection: {
                    status: "running",
                    completed_tracks: 14,
                    failed_tracks: 0,
                    skipped_tracks: 0,
                    total_tracks: 120,
                  },
                },
              },
            }),
          ]
        : [],
    );

    renderView();

    expect(await screen.findByText("Voice detection: 14 of 120 eligible tracks processed"))
      .toBeInTheDocument();
    expect(screen.getByRole("progressbar", { name: "Audio context pass progress" })).toHaveValue(
      120,
    );
    expect(screen.getByRole("progressbar", { name: "Voice detection pass progress" })).toHaveValue(
      14,
    );
    expect(screen.getByText("Interrupted")).toBeInTheDocument();
    expect(screen.getByText(/a voice worker terminated abruptly/)).toBeInTheDocument();
  });

  it("starts the one multistep analysis and explains checkpointing", async () => {
    const queued = job({ status: "queued", progress_current: 0, progress_total: null });
    vi.mocked(assistantApi.startLibraryContextAnalysis).mockResolvedValue(queued);
    const user = userEvent.setup();
    renderView();

    const panel = await screen.findByRole("region", {
      name: "Build library context",
    });
    await user.click(within(panel).getByRole("button", { name: "Build library context" }));
    await waitFor(() =>
      expect(assistantApi.startLibraryContextAnalysis).toHaveBeenCalledWith(false),
    );
    expect(toast.success).toHaveBeenCalledWith(
      "Context analysis queued",
      "The multistep analysis continues on the server and checkpoints every track.",
    );
  });

  it("keeps a full rebuild explicit", async () => {
    vi.mocked(assistantApi.startLibraryContextAnalysis).mockResolvedValue(
      job({ status: "queued", progress_current: 0, progress_total: null }),
    );
    const user = userEvent.setup();
    renderView();

    const panel = await screen.findByRole("region", {
      name: "Build library context",
    });
    await user.click(
      within(panel).getByRole("button", { name: "Rebuild all profiles" }),
    );
    await waitFor(() =>
      expect(assistantApi.startLibraryContextAnalysis).toHaveBeenCalledWith(true),
    );
  });

  it("shows bounded performance profiling for a completed context run", async () => {
    vi.mocked(jobsApi.list).mockImplementation(async (params) =>
      params?.kind === "assistant.library-context-analysis"
        ? [
            job({
              status: "succeeded",
              progress_current: 3,
              progress_total: 3,
              progress_message: "Processed 3 of 3 tracks with 3 workers",
              finished_at: "2026-08-24T10:02:00Z",
              result: {
                updated: 3,
                unchanged: 0,
                failed: 0,
                analysis_workers: 3,
                performance: {
                  schema_version: "library-context-performance/v1",
                  tracks_profiled: 3,
                  wall_seconds: 75.2,
                  worker_seconds: 210.0,
                  audio_seconds: 1_800.0,
                  audio_realtime_factor: 23.936,
                  dominant_stage: "spectrum",
                  stage_seconds: { spectrum: 125.0, voice: 60.0 },
                  stage_share_percent: { spectrum: 67.6, voice: 32.4 },
                },
              },
            }),
          ]
        : [],
    );

    renderView();

    expect(await screen.findByText("Performance profile")).toBeInTheDocument();
    expect(screen.getByText(/3 tracks profiled/)).toHaveTextContent(
      "3 tracks profiled · 1m 15s wall time · 23.9× real-time · 3 workers",
    );
    expect(
      screen.getByText("Largest measured stage: Spectrum (NumPy FFT)"),
    ).toBeInTheDocument();
    expect(screen.getByText("2m 5s · 67.6%")).toBeInTheDocument();
  });
});
