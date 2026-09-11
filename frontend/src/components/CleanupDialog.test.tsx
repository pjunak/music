import { render, screen, waitFor } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";

import type * as ApiModule from "@/core/api";
import type { BackgroundJob, CleanupAnalyzeResult, CleanupReviewProposal } from "@/core/api";

vi.mock("@/core/api", async (importActual) => {
  const actual = await importActual<typeof ApiModule>();
  return {
    ...actual,
    cleanupApi: {
      ...actual.cleanupApi,
      analyze: vi.fn(),
      enrich: vi.fn(),
      apply: vi.fn(),
      matchRejected: vi.fn(async (proposals: unknown[]) => proposals.map(() => null)),
      reject: vi.fn(),
      rejected: vi.fn(),
      restoreRejected: vi.fn(),
      forgetRejected: vi.fn(),
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
  afterEach(() => {
    vi.unstubAllGlobals();
    vi.restoreAllMocks();
  });
  beforeEach(() => {
    vi.clearAllMocks();
    vi.mocked(cleanupApi.analyze).mockResolvedValue(localResult);
    vi.mocked(cleanupApi.matchRejected).mockImplementation(async (proposals) => proposals.map(() => null));
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
    expect(screen.queryByRole("button", { name: "Download catalog results" })).not.toBeInTheDocument();
    expect(toast.warn).toHaveBeenCalledWith(
      "Catalog enrichment unavailable",
      expect.stringContaining("Local cleanup suggestions are still available"),
    );
  });

  it("downloads the original catalog job without local proposals or selection changes", async () => {
    const createObjectURL = vi.fn<(blob: Blob) => string>(() => "blob:catalog-export");
    const revokeObjectURL = vi.fn();
    vi.stubGlobal("URL", class extends URL {
      static override createObjectURL = createObjectURL;
      static override revokeObjectURL = revokeObjectURL;
    });
    let filename = "";
    vi.spyOn(HTMLAnchorElement.prototype, "click").mockImplementation(function (this: HTMLAnchorElement) {
      filename = this.download;
    });
    const user = userEvent.setup();
    renderWorkflow();
    expect(screen.queryByRole("button", { name: "Download catalog results" })).not.toBeInTheDocument();
    await user.click(screen.getByRole("button", { name: "Find issues" }));
    await screen.findByText("01_song.mp3");
    await user.click(screen.getByRole("button", { name: "None" }));
    await user.click(screen.getByRole("button", { name: "Download catalog results" }));

    const blob = createObjectURL.mock.calls[0]![0];
    const text = await new Promise<string>((resolve, reject) => {
      const reader = new FileReader();
      reader.onload = () => resolve(String(reader.result));
      reader.onerror = () => reject(reader.error);
      reader.readAsText(blob);
    });
    expect(JSON.parse(text)).toEqual(catalogJob);
    expect(blob.type).toBe("application/json");
    expect(filename).toBe("cleanup-catalog-cleanup-enrichment-1.json");
    expect(revokeObjectURL).toHaveBeenCalledWith("blob:catalog-export");
    expect(cleanupApi.enrich).toHaveBeenCalledOnce();
    expect(cleanupApi.apply).not.toHaveBeenCalled();
    expect(assistantApi.reviewAnalysisTagsBulk).not.toHaveBeenCalled();
  });

  it("retains export access when a completed run has no proposals to review", async () => {
    vi.mocked(cleanupApi.analyze).mockResolvedValue({ scanned: 0, plans: [], folders: [], pending_lookups: [] });
    vi.mocked(cleanupApi.enrich).mockResolvedValue({ ...catalogJob, result: {
      schema: "library-cleanup-enrichment/v1", scanned: 0, identified: 0,
      fingerprinted: 0, unmatched: 0, failed: 0, cached: 0, plans: [],
    } });
    const user = userEvent.setup();
    renderWorkflow();
    await user.click(screen.getByRole("button", { name: "Find issues" }));
    expect(await screen.findByRole("button", { name: "Download catalog results" })).toBeEnabled();
    expect(screen.getByRole("button", { name: "Find issues" })).toBeEnabled();
  });

  it("skips oversized catalog jobs using the scanned count and retains local review", async () => {
    vi.mocked(cleanupApi.analyze).mockResolvedValue({ ...localResult, scanned: 501 });
    const user = userEvent.setup();
    renderWorkflow();
    await user.click(screen.getByRole("button", { name: "Find issues" }));

    expect(await screen.findByText("01_song.mp3")).toBeInTheDocument();
    expect(screen.getByRole("status")).toHaveTextContent("501-track scan");
    expect(screen.getByRole("status")).toHaveTextContent("up to 500 tracks");
    expect(cleanupApi.enrich).not.toHaveBeenCalled();
    expect(cleanupApi.apply).not.toHaveBeenCalled();

    await user.click(screen.getByRole("button", { name: "Back" }));
    vi.mocked(cleanupApi.analyze).mockResolvedValue({ ...localResult, scanned: 500 });
    await user.click(screen.getByRole("button", { name: "Find issues" }));
    await waitFor(() => expect(cleanupApi.enrich).toHaveBeenCalledOnce());
    expect(screen.queryByText(/501-track scan/)).not.toBeInTheDocument();
  });

  it("explains an oversized catalog skip even when local analysis finds no issues", async () => {
    vi.mocked(cleanupApi.analyze).mockResolvedValue({ scanned: 501, plans: [], folders: [], pending_lookups: [] });
    const user = userEvent.setup();
    renderWorkflow();
    await user.click(screen.getByRole("button", { name: "Find issues" }));

    expect(await screen.findByRole("status")).toHaveTextContent("Catalog lookup was skipped");
    expect(screen.getByRole("button", { name: "Find issues" })).toBeEnabled();
    expect(cleanupApi.enrich).not.toHaveBeenCalled();
  });

  it("does not report skipped catalog work when catalogs were not requested", async () => {
    vi.mocked(cleanupApi.analyze).mockResolvedValue({ ...localResult, scanned: 501 });
    const user = userEvent.setup();
    renderWorkflow();
    await user.click(screen.getByRole("checkbox", { name: /Identify tracks and retrieve canonical metadata/ }));
    await user.click(screen.getByRole("button", { name: "Find issues" }));

    expect(await screen.findByText("01_song.mp3")).toBeInTheDocument();
    expect(screen.queryByText(/Catalog lookup was skipped/)).not.toBeInTheDocument();
    expect(cleanupApi.enrich).not.toHaveBeenCalled();
  });

  it("keeps explicit rejections in the pool and restores them unchecked before applying", async () => {
    vi.mocked(cleanupApi.analyze).mockResolvedValue({ ...localResult, scanned: 501 });
    const proposal: CleanupReviewProposal = { ...localResult.plans[0]!.ops[0]!, path: "Album/01_song.mp3", evidence: null, evidence_context: null };
    const item = { id: 12, proposal, current: true, rejected_at: 1_800_000_000 };
    vi.mocked(cleanupApi.reject).mockResolvedValue(item);
    vi.mocked(cleanupApi.rejected).mockResolvedValue({ items: [item], next_before: null });
    vi.mocked(cleanupApi.restoreRejected).mockResolvedValue(proposal);
    const user = userEvent.setup(); renderWorkflow();
    await user.click(screen.getByRole("button", { name: "Find issues" }));
    const checkbox = await screen.findByRole("checkbox", { name: /Title/ });
    expect(checkbox).toBeChecked();
    expect(screen.getByRole("status")).toHaveTextContent("501-track scan");
    await user.click(checkbox);
    expect(cleanupApi.reject).not.toHaveBeenCalled();
    await user.click(screen.getByRole("button", { name: "Reject Title suggestion for 01_song.mp3" }));
    await waitFor(() => expect(cleanupApi.reject).toHaveBeenCalledWith(proposal));
    expect(screen.getByRole("button", { name: "Apply 0 changes" })).toBeDisabled();
    await user.click(screen.getByRole("button", { name: /Rejected suggestions/ }));
    expect(await screen.findByText("Album/01_song.mp3")).toBeVisible();
    await user.click(screen.getByRole("button", { name: "Restore to review" }));
    const restored = await screen.findByRole("checkbox", { name: /Title/ });
    expect(restored).not.toBeChecked();
    expect(screen.queryByText(/501-track scan/)).not.toBeInTheDocument();
    expect(cleanupApi.apply).not.toHaveBeenCalled();
    await user.click(restored);
    await user.click(screen.getByRole("button", { name: "Apply 1 change" }));
    await waitFor(() => expect(cleanupApi.apply).toHaveBeenCalledWith([{ track_id: 7, kind: "tag", field: "title", old: "", new: "Song" }], null, "restored rejected suggestion"));
    await user.click(await screen.findByRole("button", { name: "Rejected suggestions" }));
    expect(await screen.findByRole("searchbox", { name: "Search rejected suggestions" })).toBeVisible();
  });

  it("does not select or show a previously rejected operation on a new scan", async () => {
    vi.mocked(cleanupApi.enrich).mockRejectedValue(new Error("catalog offline"));
    vi.mocked(cleanupApi.matchRejected).mockResolvedValue([12]);
    const user = userEvent.setup(); renderWorkflow();
    await user.click(screen.getByRole("button", { name: "Find issues" }));
    expect(await screen.findByRole("button", { name: "Rejected suggestions (1 hidden)" })).toBeVisible();
    expect(screen.queryByRole("checkbox", { name: /Title/ })).not.toBeInTheDocument();
    expect(screen.getByRole("button", { name: "Apply 0 changes" })).toBeDisabled();
  });

  it("keeps stale pool entries searchable and starts a selected-track recheck", async () => {
    const proposal: CleanupReviewProposal = { ...localResult.plans[0]!.ops[0]!, path: "Album/01_song.mp3", evidence: null, evidence_context: null };
    vi.mocked(cleanupApi.rejected).mockResolvedValue({ items: [{ id: 12, proposal, current: false, rejected_at: 1_800_000_000 }], next_before: 12 });
    const user = userEvent.setup(); renderWorkflow();
    await user.click(screen.getByRole("button", { name: "Rejected suggestions" }));
    expect(await screen.findByRole("button", { name: "Check again" })).toBeVisible();
    expect(screen.queryByRole("button", { name: "Restore to review" })).not.toBeInTheDocument();
    await user.type(screen.getByRole("searchbox", { name: "Search rejected suggestions" }), "Song");
    await user.click(screen.getByRole("button", { name: "Search" }));
    await waitFor(() => expect(cleanupApi.rejected).toHaveBeenLastCalledWith(null, "Song"));
    await user.click(screen.getByRole("button", { name: "Older suggestions" }));
    await waitFor(() => expect(cleanupApi.rejected).toHaveBeenLastCalledWith(12, "Song"));
    await user.click(screen.getByRole("button", { name: "Check again" }));
    await user.click(screen.getByRole("button", { name: "Find issues" }));
    await waitFor(() => expect(cleanupApi.analyze).toHaveBeenCalledWith({ type: "tracks", track_ids: [7] }, expect.any(Array)));
    expect(cleanupApi.restoreRejected).not.toHaveBeenCalled();
  });

  it("keeps a failed restore visible without applying or losing the pool entry", async () => {
    const proposal: CleanupReviewProposal = { ...localResult.plans[0]!.ops[0]!, path: "Album/01_song.mp3", evidence: null, evidence_context: null };
    vi.mocked(cleanupApi.rejected).mockResolvedValue({ items: [{ id: 12, proposal, current: true, rejected_at: 1_800_000_000 }], next_before: null });
    vi.mocked(cleanupApi.restoreRejected).mockRejectedValue(new Error("The evidence changed. Check again."));
    const user = userEvent.setup(); renderWorkflow();
    await user.click(screen.getByRole("button", { name: "Rejected suggestions" }));
    await user.click(await screen.findByRole("button", { name: "Restore to review" }));
    expect(await screen.findByRole("alert")).toHaveTextContent("The evidence changed");
    expect(screen.getByText("Album/01_song.mp3")).toBeVisible();
    expect(cleanupApi.apply).not.toHaveBeenCalled();
    expect(cleanupApi.forgetRejected).not.toHaveBeenCalled();
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
