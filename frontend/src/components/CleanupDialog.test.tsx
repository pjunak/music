import { render, screen, waitFor } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { beforeEach, describe, expect, it, vi } from "vitest";

import type * as ApiModule from "@/core/api";
import type { BackgroundJob, CleanupAnalyzeResult } from "@/core/api";

vi.mock("@/core/api", async (importActual) => {
  const actual = await importActual<typeof ApiModule>();
  return {
    ...actual,
    cleanupApi: {
      ...actual.cleanupApi,
      analyze: vi.fn(),
      enrich: vi.fn(),
      apply: vi.fn(),
    },
    assistantApi: {
      ...actual.assistantApi,
      reviewAnalysisTagsBulk: vi.fn(),
    },
  };
});

vi.mock("@/core/toast", () => ({
  toast: { success: vi.fn(), error: vi.fn(), warn: vi.fn() },
}));

import { assistantApi, cleanupApi } from "@/core/api";
import { toast } from "@/core/toast";

import { CleanupWorkflow } from "./CleanupDialog";

const localResult: CleanupAnalyzeResult = {
  scanned: 1,
  pending_lookups: [],
  folders: [],
  plans: [
    {
      track_id: 7,
      path: "Album/01_song.mp3",
      notes: [],
      ops: [
        {
          op_id: "local-title",
          track_id: 7,
          kind: "tag",
          field: "title",
          old: "",
          new: "Song",
          rules: ["tag_title"],
          confidence: "high",
          verified: false,
        },
      ],
    },
  ],
};

const catalogJob: BackgroundJob = {
  id: "cleanup-enrichment-1",
  kind: "library.cleanup-enrichment",
  status: "succeeded",
  parameters: {},
  result: {
    schema: "library-cleanup-enrichment/v1",
    scanned: 1,
    identified: 1,
    fingerprinted: 0,
    unmatched: 0,
    failed: 0,
    cached: 0,
    plans: [
      {
        schema: "library-cleanup-enrichment/v1",
        track_id: 7,
        path: "Album/01_song.mp3",
        status: "identified",
        identity: {
          recording_mbid: "recording-1",
          method: "metadata",
          confidence: 0.98,
          title: "Song",
          artist: "Artist",
          release_mbid: "release-1",
        },
        ops: [],
        tag_suggestions: [
          {
            track_id: 7,
            tag: "dark",
            analyzer_id: "catalog-tags/v1",
            source_signature: "a".repeat(64),
            source_tag: "dark",
            count: 80,
            confidence: "medium",
          },
        ],
        notes: [],
      },
    ],
  },
  error: null,
  progress_current: 1,
  progress_total: 1,
  progress_phase: "identify",
  progress_message: "Processed 1 of 1 tracks",
  attempts: 1,
  retry_of_id: null,
  created_at: "2026-09-01T00:00:00Z",
  updated_at: "2026-09-01T00:00:01Z",
  started_at: "2026-09-01T00:00:00Z",
  finished_at: "2026-09-01T00:00:01Z",
};

function renderWorkflow(onApplied = vi.fn()) {
  render(
    <CleanupWorkflow
      path=""
      checkedIds={[]}
      onClose={vi.fn()}
      onApplied={onApplied}
      presentation="workspace"
    />,
  );
  return onApplied;
}

describe("CleanupWorkflow catalog enrichment", () => {
  beforeEach(() => {
    vi.clearAllMocks();
    vi.mocked(cleanupApi.analyze).mockResolvedValue(localResult);
    vi.mocked(cleanupApi.enrich).mockResolvedValue(catalogJob);
    vi.mocked(cleanupApi.apply).mockResolvedValue({ batch_id: 3, applied: 1, skipped: [] });
    vi.mocked(assistantApi.reviewAnalysisTagsBulk).mockResolvedValue({
      requested_items: 1,
      applied: [
        {
          track_id: 7,
          tag: "dark",
          analyzer_id: "catalog-tags/v1",
          source_signature: "a".repeat(64),
          decision: "accepted",
        },
      ],
      failures: [],
    });
  });

  it("keeps local suggestions reviewable when a catalog request is unavailable", async () => {
    vi.mocked(cleanupApi.enrich).mockRejectedValue(new Error("catalog offline"));
    const user = userEvent.setup();
    renderWorkflow();

    await user.click(screen.getByRole("button", { name: "Find issues" }));

    expect(await screen.findByText("01_song.mp3")).toBeInTheDocument();
    expect(screen.getByText("Song")).toBeInTheDocument();
    expect(toast.warn).toHaveBeenCalledWith(
      "Catalog enrichment unavailable",
      expect.stringContaining("Local cleanup suggestions are still available"),
    );
  });

  it("keeps catalog mood tags unticked until they are explicitly accepted", async () => {
    vi.mocked(cleanupApi.analyze).mockResolvedValue({ ...localResult, plans: [] });
    const onApplied = renderWorkflow();
    const user = userEvent.setup();

    await user.click(screen.getByRole("button", { name: "Find issues" }));

    expect(await screen.findByText("Database mood tag suggestions")).toBeInTheDocument();
    const tagCheckbox = screen.getByRole("checkbox");
    expect(tagCheckbox).not.toBeChecked();
    await user.click(tagCheckbox);
    await user.click(screen.getByRole("button", { name: "Apply 1 change" }));

    await waitFor(() =>
      expect(assistantApi.reviewAnalysisTagsBulk).toHaveBeenCalledWith(
        [
          {
            track_id: 7,
            tag: "dark",
            analyzer_id: "catalog-tags/v1",
            source_signature: "a".repeat(64),
          },
        ],
        "accepted",
      ),
    );
    expect(cleanupApi.apply).not.toHaveBeenCalled();
    expect(onApplied).toHaveBeenCalled();
  });

  it("accepts selected catalog tags before metadata changes make their evidence stale", async () => {
    const user = userEvent.setup();
    renderWorkflow();

    await user.click(screen.getByRole("button", { name: "Find issues" }));
    await screen.findByText("Database mood tag suggestions");
    const catalogTag = screen
      .getAllByRole("checkbox")
      .find((checkbox) => !(checkbox as HTMLInputElement).checked);
    expect(catalogTag).toBeDefined();
    await user.click(catalogTag as HTMLElement);
    await user.click(screen.getByRole("button", { name: "Apply 2 changes" }));

    await waitFor(() => expect(cleanupApi.apply).toHaveBeenCalled());
    expect(
      vi.mocked(assistantApi.reviewAnalysisTagsBulk).mock.invocationCallOrder[0],
    ).toBeLessThan(vi.mocked(cleanupApi.apply).mock.invocationCallOrder[0]);
  });
});
