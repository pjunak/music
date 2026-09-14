import { render, screen } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { afterEach, expect, it, vi } from "vitest";
import { cleanupApi } from "@/core/api";
import { CleanupModelReview } from "./CleanupModelReview";

afterEach(() => vi.restoreAllMocks());

it("requires explicit disclosure consent and resets it after a failed request", async () => {
  const user = userEvent.setup();
  vi.spyOn(cleanupApi, "modelStatus").mockResolvedValue({ available: true, reason_code: null, model_id: "fixture", disclosure_version: "v1", shared_with_provider: ["Title and candidate facts"], never_shared: ["Audio and paths"], maximum_candidates: 25, may_incur_cost: true });
  const call = vi.spyOn(cleanupApi, "reviewCandidates").mockRejectedValue(new Error("Unavailable"));
  render(<CleanupModelReview trackId={7} catalogJobId="catalog-job" onResult={vi.fn()} />);
  await user.click(screen.getByRole("button", { name: "Review ambiguous candidates with AI" }));
  const run = await screen.findByRole("button", { name: "Ask AI to review" });
  expect(run).toBeDisabled();
  expect(call).not.toHaveBeenCalled();
  await user.click(screen.getByRole("checkbox"));
  await user.click(run);
  expect(call).toHaveBeenCalledExactlyOnceWith(7, "catalog-job", "v1", false);
  expect(await screen.findByRole("button", { name: "Ask AI to review" })).toBeDisabled();
});

it("discloses and explicitly dispatches an edition review without applying its advice", async () => {
  const user = userEvent.setup();
  vi.spyOn(cleanupApi, "modelStatus").mockResolvedValue({ available: true, reason_code: null, model_id: "fixture", disclosure_version: "v2", shared_with_provider: ["Edition descriptions and distinguishing song titles"], never_shared: ["Audio and paths"], maximum_candidates: 25, may_incur_cost: true });
  const call = vi.spyOn(cleanupApi, "reviewCandidates").mockRejectedValue(new Error("Unavailable"));
  const onResult = vi.fn();
  render(<CleanupModelReview trackId={7} catalogJobId="catalog-job" editionReview onResult={onResult} />);
  await user.click(screen.getByRole("button", { name: "Ask AI about these editions" }));
  const run = screen.getByRole("button", { name: "Ask AI to review" });
  expect(run).toBeDisabled();
  expect(screen.getByText(/Edition descriptions and distinguishing song titles/)).toBeInTheDocument();
  await user.click(screen.getByRole("checkbox"));
  await user.click(run);
  expect(call).toHaveBeenCalledExactlyOnceWith(7, "catalog-job", "v2", true);
  expect(onResult).not.toHaveBeenCalled();
  expect(await screen.findByRole("button", { name: "Ask AI to review" })).toBeDisabled();
});
