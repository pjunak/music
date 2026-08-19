import { render, screen, waitFor } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { MemoryRouter } from "react-router-dom";
import { beforeEach, describe, expect, it, vi } from "vitest";

import type * as ApiModule from "@/core/api";
import type {
  BackgroundJob,
  ModelTaggingAvailability,
} from "@/core/api";

vi.mock("@/core/api", async (importActual) => {
  const actual = await importActual<typeof ApiModule>();
  return {
    ...actual,
    assistantApi: {
      ...actual.assistantApi,
      getModelTaggingAvailability: vi.fn(),
      startModelTagging: vi.fn(),
    },
    jobsApi: {
      list: vi.fn(),
      get: vi.fn(),
      cancel: vi.fn(),
      retry: vi.fn(),
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
import {
  MODEL_TAGGING_DISCLOSURE_VERSION,
  assistantApi,
  jobsApi,
} from "@/core/api";
import { toast } from "@/core/toast";

import { ModelTaggingPanel } from "./ModelTaggingPanel";

const availability: ModelTaggingAvailability = {
  available: true,
  reason_code: null,
  role_id: "music_tagger",
  connection_name: "Hosted models",
  model_id: "tagger-small",
  quality_evaluation_id: "music-tagging-quality-v1",
  job_kind: "assistant.model-music-tagging",
  library_tracks: 45,
  tracks_with_audio_evidence: 32,
  current_profiles: 5,
  tracks_needing_tags: 40,
  estimated_provider_requests: 2,
  disclosure: {
    version: MODEL_TAGGING_DISCLOSURE_VERSION,
    shared_with_provider: ["Indexed titles, artists, albums, origins, and genres"],
    never_shared: ["Audio files", "Filesystem paths", "Your manual tags"],
    allowed_tags: ["medieval", "tavern", "dancing"],
    tracks_per_request: 20,
    may_incur_cost: true,
  },
};

function taggingJob(overrides: Partial<BackgroundJob> = {}): BackgroundJob {
  return {
    id: "tag-job-1",
    kind: "assistant.model-music-tagging",
    status: "running",
    parameters: {
      role_id: "music_tagger",
      disclosure_version: MODEL_TAGGING_DISCLOSURE_VERSION,
      force: false,
    },
    result: null,
    error: null,
    progress_current: 20,
    progress_total: 40,
    progress_phase: "Waiting for music tagging model",
    progress_message: "Processed 20 of 40 tracks",
    attempts: 1,
    retry_of_id: null,
    created_at: "2026-08-19T12:00:00Z",
    updated_at: "2026-08-19T12:01:00Z",
    started_at: "2026-08-19T12:00:01Z",
    finished_at: null,
    ...overrides,
  };
}

function renderPanel(onSuggestionsChanged = vi.fn()) {
  render(
    <MemoryRouter>
      <ModelTaggingPanel onSuggestionsChanged={onSuggestionsChanged} />
    </MemoryRouter>,
  );
  return onSuggestionsChanged;
}

beforeEach(() => {
  vi.clearAllMocks();
  vi.mocked(assistantApi.getModelTaggingAvailability).mockResolvedValue(
    availability,
  );
  vi.mocked(jobsApi.list).mockResolvedValue([]);
  vi.mocked(confirmDialog).mockResolvedValue(true);
});

describe("ModelTaggingPanel", () => {
  it("shows the provider boundary and starts only after exact consent", async () => {
    const queued = taggingJob({
      status: "queued",
      progress_current: 0,
      progress_total: null,
      progress_phase: "Queued",
      progress_message: "",
      started_at: null,
    });
    vi.mocked(assistantApi.startModelTagging).mockResolvedValue(queued);
    const user = userEvent.setup();
    renderPanel();

    expect(
      await screen.findByRole("heading", {
        name: "Suggest D&D tags from library evidence",
      }),
    ).toBeInTheDocument();
    expect(screen.getByText("Filesystem paths")).toBeInTheDocument();
    await user.click(
      screen.getByRole("button", { name: "Suggest tags for 40 tracks" }),
    );

    expect(confirmDialog).toHaveBeenCalledWith(
      expect.objectContaining({
        title: "Send library evidence to your tagging model?",
        confirmLabel: "Suggest tags",
      }),
    );
    await waitFor(() =>
      expect(assistantApi.startModelTagging).toHaveBeenCalledWith(
        false,
        MODEL_TAGGING_DISCLOSURE_VERSION,
      ),
    );
    expect(toast.success).toHaveBeenCalledWith(
      "Model tagging queued",
      "You can close this page; progress and completed suggestions are stored on the server.",
    );
  });

  it("restores server progress after reopen and can cancel", async () => {
    const running = taggingJob();
    vi.mocked(assistantApi.getModelTaggingAvailability).mockRejectedValue(
      new Error("Model readiness is temporarily unavailable."),
    );
    vi.mocked(jobsApi.list).mockResolvedValue([running]);
    vi.mocked(jobsApi.cancel).mockResolvedValue({
      ...running,
      status: "cancel_requested",
      progress_phase: "Cancelling",
    });
    const user = userEvent.setup();
    renderPanel();

    expect(await screen.findByText("Processed 20 of 40 tracks")).toBeInTheDocument();
    expect(
      screen.getByText("Model readiness is temporarily unavailable."),
    ).toBeInTheDocument();
    expect(
      screen.getByRole("progressbar", { name: "Model music tagging progress" }),
    ).toHaveValue(20);
    expect(screen.getByText(/Safe to close/)).toBeInTheDocument();

    await user.click(screen.getByRole("button", { name: "Cancel run" }));
    expect(jobsApi.cancel).toHaveBeenCalledWith("tag-job-1");
  });

  it("makes a full-library rebuild explicit and updates its cost estimate", async () => {
    const queued = taggingJob({
      status: "queued",
      progress_current: 0,
      progress_total: null,
      progress_phase: "Queued",
      progress_message: "",
      started_at: null,
      parameters: {
        role_id: "music_tagger",
        disclosure_version: MODEL_TAGGING_DISCLOSURE_VERSION,
        force: true,
      },
    });
    vi.mocked(assistantApi.startModelTagging).mockResolvedValue(queued);
    const user = userEvent.setup();
    renderPanel();

    await user.click(
      await screen.findByLabelText("Rebuild every model profile"),
    );
    expect(
      screen.getByRole("button", { name: "Suggest tags for 45 tracks" }),
    ).toBeInTheDocument();
    expect(screen.getByText(/about 3 provider requests/)).toBeInTheDocument();
    await user.click(
      screen.getByRole("button", { name: "Suggest tags for 45 tracks" }),
    );

    await waitFor(() =>
      expect(assistantApi.startModelTagging).toHaveBeenCalledWith(
        true,
        MODEL_TAGGING_DISCLOSURE_VERSION,
      ),
    );
  });

  it("refreshes review suggestions when a completed run is restored", async () => {
    const completed = taggingJob({
      status: "succeeded",
      progress_current: 40,
      result: {
        schema_version: "assistant-model-music-tagging-job-result/v2",
        analyzer_id: "model-evidence-tagger/v2",
        library_tracks: 45,
        updated_profiles: 40,
        unchanged_profiles: 5,
        skipped_changed_tracks: 0,
      },
      progress_phase: "Complete",
      progress_message: "Processed 40 of 40 tracks",
      finished_at: "2026-08-19T12:05:00Z",
    });
    vi.mocked(jobsApi.list).mockResolvedValue([completed]);
    const onSuggestionsChanged = renderPanel();

    expect(
      await screen.findByText("Generated suggestions are ready for review"),
    ).toBeInTheDocument();
    expect(screen.getByText(/Updated 40 profiles/)).toBeInTheDocument();
    await waitFor(() => expect(onSuggestionsChanged).toHaveBeenCalledTimes(1));
  });
});
