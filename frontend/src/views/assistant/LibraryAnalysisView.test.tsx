import { render, screen, waitFor } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { beforeEach, describe, expect, it, vi } from "vitest";

import type * as ApiModule from "@/core/api";
import type {
  BackgroundJob,
  LibraryAnalysisSummary,
  LibraryTagPage,
  ManualTagCatalog,
} from "@/core/api";

vi.mock("@/core/api", async (importActual) => {
  const actual = await importActual<typeof ApiModule>();
  return {
    ...actual,
    assistantApi: {
      ...actual.assistantApi,
      startLibraryAnalysis: vi.fn(),
      getLibraryAnalysisSummary: vi.fn(),
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

const summary: LibraryAnalysisSummary = {
  analyzer: "local-metadata/v1",
  library_tracks: 120,
  analyzed_tracks: 80,
  high_confidence: 35,
  medium_confidence: 30,
  low_confidence: 15,
  last_updated_at: "2026-08-18T10:00:00Z",
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
  vi.mocked(assistantApi.getManualTagCatalog).mockResolvedValue(tagCatalog);
  vi.mocked(assistantApi.listLibraryTags).mockResolvedValue(emptyTagPage);
  vi.mocked(jobsApi.list).mockResolvedValue([]);
});

describe("LibraryAnalysisView", () => {
  it("restores persisted progress and can request cancellation", async () => {
    const running = job();
    vi.mocked(jobsApi.list).mockResolvedValue([running]);
    vi.mocked(jobsApi.cancel).mockResolvedValue({
      ...running,
      status: "cancel_requested",
      progress_phase: "Cancelling",
    });
    const user = userEvent.setup();
    render(<LibraryAnalysisView />);

    expect(await screen.findByText("Processed 42 of 120 tracks")).toBeInTheDocument();
    expect(screen.getByRole("progressbar", { name: "Library analysis progress" })).toHaveValue(
      42,
    );
    expect(screen.getByText("80")).toBeInTheDocument();
    expect(screen.getByText("Analyzed")).toBeInTheDocument();

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
    render(<LibraryAnalysisView />);

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
    render(<LibraryAnalysisView />);

    await screen.findByText("No library analysis has run yet");
    await user.click(screen.getByRole("button", { name: "Rebuild all profiles" }));

    await waitFor(() => expect(assistantApi.startLibraryAnalysis).toHaveBeenCalledWith(true));
    expect(toast.success).toHaveBeenCalledWith(
      "Library rebuild queued",
      "You can leave this page; progress is stored on the server.",
    );
  });
});
