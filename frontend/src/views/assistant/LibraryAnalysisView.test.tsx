import { render, screen, waitFor, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { MemoryRouter } from "react-router-dom";
import { beforeEach, describe, expect, it, vi } from "vitest";

import type * as ApiModule from "@/core/api";
import type {
  BackgroundJob,
  LibraryAnalysisSummary,
  LibraryTagPage,
  ManualTagCatalog,
  ModelTagCleanupAvailability,
  ModelTaggingAvailability,
} from "@/core/api";

vi.mock("@/core/api", async (importActual) => {
  const actual = await importActual<typeof ApiModule>();
  return {
    ...actual,
    assistantApi: {
      ...actual.assistantApi,
      startLibraryAnalysis: vi.fn(),
      getLibraryAnalysisSummary: vi.fn(),
      startLibraryAudioAnalysis: vi.fn(),
      getLibraryAudioAnalysisSummary: vi.fn(),
      getModelTaggingAvailability: vi.fn(),
      startModelTagging: vi.fn(),
      getModelTagCleanupAvailability: vi.fn(),
      startModelTagCleanup: vi.fn(),
      applyModelTagCleanup: vi.fn(),
      getManualTagCatalog: vi.fn(),
      listLibraryTags: vi.fn(),
      patchManualTags: vi.fn(),
      patchManualTagsBulk: vi.fn(),
      renameManualTag: vi.fn(),
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
  toast: {
    success: vi.fn(),
    error: vi.fn(),
  },
}));

import { assistantApi, jobsApi } from "@/core/api";
import { toast } from "@/core/toast";

import { LibraryAnalysisView } from "./LibraryAnalysisView";

function renderView() {
  return render(
    <MemoryRouter>
      <LibraryAnalysisView />
    </MemoryRouter>,
  );
}

const summary: LibraryAnalysisSummary = {
  analyzer: "local-metadata/v1",
  library_tracks: 120,
  analyzed_tracks: 80,
  failed_tracks: 0,
  stale_tracks: 0,
  high_confidence: 35,
  medium_confidence: 30,
  low_confidence: 15,
  last_updated_at: "2026-08-18T10:00:00Z",
};

const audioSummary: LibraryAnalysisSummary = {
  analyzer: "local-audio/v1",
  library_tracks: 120,
  analyzed_tracks: 64,
  failed_tracks: 2,
  stale_tracks: 1,
  high_confidence: 24,
  medium_confidence: 30,
  low_confidence: 10,
  last_updated_at: "2026-08-18T11:00:00Z",
};

const emptyTagPage: LibraryTagPage = {
  items: [],
  total: 0,
  offset: 0,
  limit: 50,
};

const tagCatalog: ManualTagCatalog = {
  starter_groups: [],
  used_tags: [],
  tag_usage: [],
};

const modelTaggingUnavailable: ModelTaggingAvailability = {
  available: false,
  reason_code: "role_not_configured",
  role_id: "music_tagger",
  connection_name: null,
  model_id: null,
  quality_evaluation_id: "music-tagging-quality-v1",
  job_kind: "assistant.model-music-tagging",
  library_tracks: 120,
  scope_tracks: 120,
  tracks_with_audio_evidence: 0,
  current_profiles: 0,
  tracks_needing_tags: 120,
  estimated_provider_requests: 6,
  disclosure: {
    version: "assistant-model-music-tagging-disclosure/v5",
    shared_with_provider: [],
    never_shared: [],
    allowed_tags: ["medieval", "tavern"],
    tracks_per_request: 20,
    may_incur_cost: true,
  },
};

const modelTagCleanupUnavailable: ModelTagCleanupAvailability = {
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
};

function job(overrides: Partial<BackgroundJob> = {}): BackgroundJob {
  return {
    id: "job-1",
    kind: "assistant.library-analysis",
    status: "running",
    parameters: { force: false },
    result: null,
    error: null,
    progress_current: 42,
    progress_total: 120,
    progress_phase: "Profiling library",
    progress_message: "Processed 42 of 120 tracks",
    attempts: 1,
    retry_of_id: null,
    created_at: "2026-08-18T10:00:00Z",
    updated_at: "2026-08-18T10:01:00Z",
    started_at: "2026-08-18T10:00:01Z",
    finished_at: null,
    ...overrides,
  };
}

beforeEach(() => {
  vi.clearAllMocks();
  vi.mocked(assistantApi.getLibraryAnalysisSummary).mockResolvedValue(summary);
  vi.mocked(assistantApi.getLibraryAudioAnalysisSummary).mockResolvedValue(
    audioSummary,
  );
  vi.mocked(assistantApi.getManualTagCatalog).mockResolvedValue(tagCatalog);
  vi.mocked(assistantApi.listLibraryTags).mockResolvedValue(emptyTagPage);
  vi.mocked(assistantApi.getModelTaggingAvailability).mockResolvedValue(
    modelTaggingUnavailable,
  );
  vi.mocked(assistantApi.getModelTagCleanupAvailability).mockResolvedValue(
    modelTagCleanupUnavailable,
  );
  vi.mocked(jobsApi.list).mockResolvedValue([]);
});

describe("LibraryAnalysisView", () => {
  it("restores persisted progress and can request cancellation", async () => {
    const running = job();
    vi.mocked(jobsApi.list).mockImplementation(async (params) =>
      params?.kind === "assistant.library-analysis" ? [running] : [],
    );
    vi.mocked(jobsApi.cancel).mockResolvedValue({
      ...running,
      status: "cancel_requested",
      progress_phase: "Cancelling",
    });
    const user = userEvent.setup();
    renderView();

    expect(await screen.findByText("Processed 42 of 120 tracks")).toBeInTheDocument();
    expect(screen.getByRole("progressbar", { name: "Library analysis progress" })).toHaveValue(
      42,
    );
    expect(screen.getByText("80")).toBeInTheDocument();
    expect(screen.getAllByText("Analyzed")).toHaveLength(2);

    await user.click(screen.getByRole("button", { name: "Cancel" }));
    expect(jobsApi.cancel).toHaveBeenCalledWith("job-1");
  });

  it("starts an incremental analysis and explains that it survives navigation", async () => {
    const queued = job({
      status: "queued",
      progress_current: 0,
      progress_total: null,
      progress_phase: "Queued",
      progress_message: "",
      started_at: null,
    });
    vi.mocked(assistantApi.startLibraryAnalysis).mockResolvedValue(queued);
    const user = userEvent.setup();
    renderView();

    await screen.findByText("No library analysis has run yet");
    await user.click(screen.getByRole("button", { name: "Analyze library" }));

    await waitFor(() =>
      expect(assistantApi.startLibraryAnalysis).toHaveBeenCalledWith(false),
    );
    expect(toast.success).toHaveBeenCalledWith(
      "Library analysis queued",
      "You can leave this page; progress is stored on the server.",
    );
  });

  it("allows an explicit full rebuild without making it the default", async () => {
    const queued = job({ status: "queued", progress_current: 0, progress_total: null });
    vi.mocked(assistantApi.startLibraryAnalysis).mockResolvedValue(queued);
    const user = userEvent.setup();
    renderView();

    await screen.findByText("No library analysis has run yet");
    const metadataPanel = screen.getByRole("region", { name: "Metadata profiles" });
    await user.click(
      within(metadataPanel).getByRole("button", { name: "Rebuild all profiles" }),
    );

    await waitFor(() => expect(assistantApi.startLibraryAnalysis).toHaveBeenCalledWith(true));
    expect(toast.success).toHaveBeenCalledWith(
      "Library rebuild queued",
      "You can leave this page; progress is stored on the server.",
    );
  });

  it("starts the separate audio signal analyzer without changing metadata tags", async () => {
    const queued = job({
      id: "audio-job",
      kind: "assistant.library-audio-analysis",
      status: "queued",
      progress_current: 0,
      progress_total: null,
      progress_phase: "Queued",
      started_at: null,
    });
    vi.mocked(assistantApi.startLibraryAudioAnalysis).mockResolvedValue(queued);
    const user = userEvent.setup();
    renderView();

    const audioPanel = await screen.findByRole("region", {
      name: "Audio signal profiles",
    });
    await user.click(
      within(audioPanel).getByRole("button", { name: "Analyze audio signals" }),
    );

    await waitFor(() =>
      expect(assistantApi.startLibraryAudioAnalysis).toHaveBeenCalledWith(false),
    );
    expect(toast.success).toHaveBeenCalledWith(
      "Audio analysis queued",
      "You can leave this page; progress is stored on the server.",
    );
  });
});
