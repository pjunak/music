import { render, screen, waitFor } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { MemoryRouter } from "react-router-dom";
import { beforeEach, describe, expect, it, vi } from "vitest";

import type * as ApiModule from "@/core/api";
import type {
  BackgroundJob,
  ModelTagCleanupAvailability,
} from "@/core/api";

vi.mock("@/core/api", async (importActual) => {
  const actual = await importActual<typeof ApiModule>();
  return {
    ...actual,
    assistantApi: {
      ...actual.assistantApi,
      getModelTagCleanupAvailability: vi.fn(),
      startModelTagCleanup: vi.fn(),
      applyModelTagCleanup: vi.fn(),
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
  MODEL_TAG_CLEANUP_DISCLOSURE_VERSION,
  assistantApi,
  jobsApi,
} from "@/core/api";

import { ModelTagCleanupPanel } from "./ModelTagCleanupPanel";

const catalogSignature = "a".repeat(64);

const availability: ModelTagCleanupAvailability = {
  available: true,
  reason_code: null,
  role_id: "tag_cleanup",
  connection_name: "Cleanup provider",
  model_id: "catalog-reviewer",
  quality_evaluation_id: "tag-cleanup-quality-v1",
  job_kind: "assistant.model-tag-cleanup",
  catalog_signature: catalogSignature,
  manual_tags: 17,
  estimated_provider_requests: 1,
  disclosure: {
    version: MODEL_TAG_CLEANUP_DISCLOSURE_VERSION,
    shared_with_provider: ["Your normalized manual tag names"],
    never_shared: ["Audio or media files", "Track titles and filesystem paths"],
    maximum_tags: 500,
    may_incur_cost: true,
  },
};

function cleanupJob(overrides: Partial<BackgroundJob> = {}): BackgroundJob {
  return {
    id: "cleanup-job-1",
    kind: "assistant.model-tag-cleanup",
    status: "queued",
    parameters: {
      role_id: "tag_cleanup",
      disclosure_version: MODEL_TAG_CLEANUP_DISCLOSURE_VERSION,
    },
    result: null,
    error: null,
    progress_current: 0,
    progress_total: 1,
    progress_phase: "Queued",
    progress_message: "",
    attempts: 1,
    retry_of_id: null,
    created_at: "2026-08-19T12:00:00Z",
    updated_at: "2026-08-19T12:00:00Z",
    started_at: null,
    finished_at: null,
    ...overrides,
  };
}

function completedJob(
  overrides: Partial<BackgroundJob> = {},
): BackgroundJob {
  return cleanupJob({
    status: "succeeded",
    progress_current: 1,
    progress_phase: "Saving cleanup proposal",
    progress_message: "Saved 1 review-only suggestion",
    started_at: "2026-08-19T12:00:01Z",
    finished_at: "2026-08-19T12:00:03Z",
    result: {
      schema_version: "assistant-model-tag-cleanup-job-result/v2",
      disclosure_version: MODEL_TAG_CLEANUP_DISCLOSURE_VERSION,
      role_id: "tag_cleanup",
      role_fingerprint: "b".repeat(64),
      engine_id: "model-tag-cleanup/v2",
      catalog_signature: catalogSignature,
      catalog_tags: 17,
      suggestions: [
        {
          id: "c".repeat(64),
          source: "tavarn",
          target: "tavern",
          origin: "local-rule",
          confidence: "high",
          reason: "Likely spelling variant of the starter tag.",
          source_track_count: 3,
          target_track_count: 8,
          merged: true,
        },
      ],
      usage: {
        provider_requests: 1,
        input_tokens: 120,
        output_tokens: 24,
      },
    },
    ...overrides,
  });
}

function renderPanel(onCatalogChanged = vi.fn()) {
  render(
    <MemoryRouter>
      <ModelTagCleanupPanel onCatalogChanged={onCatalogChanged} />
    </MemoryRouter>,
  );
  return onCatalogChanged;
}

beforeEach(() => {
  vi.clearAllMocks();
  vi.mocked(assistantApi.getModelTagCleanupAvailability).mockResolvedValue(
    availability,
  );
  vi.mocked(jobsApi.list).mockResolvedValue([]);
  vi.mocked(confirmDialog).mockResolvedValue(true);
});

describe("ModelTagCleanupPanel", () => {
  it("shows the narrow provider boundary and requires exact consent", async () => {
    vi.mocked(assistantApi.startModelTagCleanup).mockResolvedValue(cleanupJob());
    const user = userEvent.setup();
    renderPanel();

    expect(
      await screen.findByRole("heading", {
        name: "Review manual tag consistency",
      }),
    ).toBeInTheDocument();
    expect(screen.getByText("Audio or media files")).toBeInTheDocument();
    await user.click(
      screen.getByRole("button", { name: "Review 17 manual tags" }),
    );

    expect(confirmDialog).toHaveBeenCalledWith(
      expect.objectContaining({
        title: "Send your manual tag catalog to the cleanup model?",
        body: expect.stringContaining("No songs, audio, titles"),
        confirmLabel: "Request cleanup suggestions",
      }),
    );
    await waitFor(() =>
      expect(assistantApi.startModelTagCleanup).toHaveBeenCalledWith(
        MODEL_TAG_CLEANUP_DISCLOSURE_VERSION,
      ),
    );
  });

  it("restores a proposal with nothing selected and applies only a checked rename", async () => {
    vi.mocked(jobsApi.list).mockResolvedValue([completedJob()]);
    vi.mocked(assistantApi.applyModelTagCleanup).mockResolvedValue({
      schema_version: "assistant-tag-cleanup-apply/v1",
      requested_items: 1,
      applied: [
        {
          source: "tavarn",
          target: "tavern",
          affected_tracks: 3,
          merged: true,
        },
      ],
      catalog_signature: "d".repeat(64),
    });
    const onCatalogChanged = vi.fn();
    const user = userEvent.setup();
    renderPanel(onCatalogChanged);

    const applyButton = await screen.findByRole("button", {
      name: "Apply 0 selected renames",
    });
    expect(applyButton).toBeDisabled();
    await user.click(screen.getByLabelText("Select tavarn to tavern"));
    await user.click(
      screen.getByRole("button", { name: "Apply 1 selected rename" }),
    );

    expect(confirmDialog).toHaveBeenCalledWith(
      expect.objectContaining({
        title: "Apply 1 selected tag rename?",
        body: expect.stringContaining("tavarn → tavern"),
      }),
    );
    await waitFor(() =>
      expect(assistantApi.applyModelTagCleanup).toHaveBeenCalledWith(
        "cleanup-job-1",
        catalogSignature,
        [{ source: "tavarn", target: "tavern" }],
      ),
    );
    expect(onCatalogChanged).toHaveBeenCalledTimes(1);
  });

  it("rejects a restored proposal after the manual tag catalog changed", async () => {
    vi.mocked(assistantApi.getModelTagCleanupAvailability).mockResolvedValue({
      ...availability,
      catalog_signature: "e".repeat(64),
    });
    vi.mocked(jobsApi.list).mockResolvedValue([completedJob()]);
    renderPanel();

    expect(
      await screen.findByText(/catalog changed after this proposal was created/i),
    ).toBeInTheDocument();
    expect(screen.queryByLabelText("Select tavarn to tavern")).not.toBeInTheDocument();
    expect(
      screen.getByRole("button", { name: "Apply 0 selected renames" }),
    ).toBeDisabled();
  });

  it("restores server progress after reopen and can cancel", async () => {
    const running = cleanupJob({
      status: "running",
      progress_phase: "Waiting for tag cleanup model",
      progress_message: "The provider is reviewing tag names and usage counts",
    });
    vi.mocked(jobsApi.list).mockResolvedValue([running]);
    vi.mocked(jobsApi.cancel).mockResolvedValue({
      ...running,
      status: "cancel_requested",
      progress_phase: "Cancelling",
    });
    const user = userEvent.setup();
    renderPanel();

    expect(
      await screen.findByText("The provider is reviewing tag names and usage counts"),
    ).toBeInTheDocument();
    expect(screen.getByText(/Safe to close/)).toBeInTheDocument();
    await user.click(screen.getByRole("button", { name: "Cancel review" }));
    expect(jobsApi.cancel).toHaveBeenCalledWith("cleanup-job-1");
  });
});
