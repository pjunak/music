import { render, screen, waitFor } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { beforeEach, describe, expect, it, vi } from "vitest";
import { assistantApi, type ModelBatchStatus } from "@/core/api";
import { downloadJson } from "@/views/assistant/downloadJson";
import { confirmDialog } from "./confirmDialog";
import { ModelBatchStatusPanel } from "./ModelBatchStatusPanel";

vi.mock("@/core/api", () => ({
  assistantApi: { getModelBatch: vi.fn(), updateModelBatch: vi.fn() },
  jobsApi: { get: vi.fn() },
}));
vi.mock("./confirmDialog", () => ({ confirmDialog: vi.fn() }));
vi.mock("@/views/assistant/downloadJson", () => ({ downloadJson: vi.fn() }));

const pending: ModelBatchStatus = {
  id: "run", state: "submitting", input_file_id: "file-owned", remote_batch_id: null, result: null,
};

describe("ModelBatchStatusPanel", () => {
  beforeEach(() => {
    vi.clearAllMocks();
    vi.mocked(assistantApi.getModelBatch).mockResolvedValue(pending);
  });

  it("reads status without resubmitting and sends a recovery ID only on an explicit action", async () => {
    const user = userEvent.setup();
    vi.mocked(assistantApi.updateModelBatch).mockResolvedValue({ status: "succeeded" } as Awaited<ReturnType<typeof assistantApi.updateModelBatch>>);
    render(<ModelBatchStatusPanel id="run" />);
    const input = await screen.findByRole("textbox");
    expect(screen.queryByRole("button", { name: "Export retained run results" })).not.toBeInTheDocument();
    expect(assistantApi.updateModelBatch).not.toHaveBeenCalled();
    await user.type(input, "batch-owned");
    await user.click(screen.getByRole("button", { name: "Check and collect results" }));
    expect(assistantApi.updateModelBatch).toHaveBeenCalledWith("run", false, "batch-owned", false);
  });

  it("requires the explicit uncertainty acknowledgement before abandoning", async () => {
    const user = userEvent.setup();
    vi.mocked(confirmDialog).mockResolvedValueOnce(false).mockResolvedValueOnce(true);
    vi.mocked(assistantApi.updateModelBatch).mockResolvedValue({ status: "succeeded" } as Awaited<ReturnType<typeof assistantApi.updateModelBatch>>);
    render(<ModelBatchStatusPanel id="run" />);
    const abandon = await screen.findByRole("button", { name: "Abandon after checking provider account" });
    await user.click(abandon);
    expect(assistantApi.updateModelBatch).not.toHaveBeenCalled();
    await user.click(abandon);
    await waitFor(() => expect(assistantApi.updateModelBatch).toHaveBeenCalledWith("run", true, undefined, true));
  });

  it.each(["failed", "expired", "cancelled"] as const)("exports a %s batch with no retained results without resubmitting", async (state) => {
    vi.mocked(assistantApi.getModelBatch).mockResolvedValue({ ...pending, state });
    render(<ModelBatchStatusPanel id="run" />);
    const exported = await screen.findByRole("button", { name: "Export retained run results" });
    expect(screen.getAllByRole("button")).toEqual([exported]);
    await userEvent.click(exported);
    expect(downloadJson).toHaveBeenCalledWith({
      schema_version: "assistant-mood-run-export/v1", run_id: "run", track_results: [], usage: null,
    }, "mood-run-run.json");
    expect(assistantApi.updateModelBatch).not.toHaveBeenCalled();
  });

  it("keeps completed suggestions review-only and offers no repeat submission", async () => {
    vi.mocked(assistantApi.getModelBatch).mockResolvedValue({ ...pending, state: "completed", result: { updated_profiles: 3, rejected_tracks: 1 } });
    render(<ModelBatchStatusPanel id="run" />);
    expect(await screen.findByText(/Saved 3 profiles/)).toHaveTextContent(/still require review/);
    const exported = screen.getByRole("button", { name: "Export retained run results" });
    expect(screen.getAllByRole("button")).toEqual([exported]);
    await userEvent.click(exported);
    expect(downloadJson).toHaveBeenCalledWith({
      schema_version: "assistant-mood-run-export/v1", run_id: "run", track_results: [], usage: null,
    }, "mood-run-run.json");
    expect(assistantApi.updateModelBatch).not.toHaveBeenCalled();
  });
});
