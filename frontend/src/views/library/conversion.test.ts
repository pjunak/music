import { afterEach, expect, it, vi } from "vitest";

import { jobsApi } from "@/core/api";
import type { BackgroundJob } from "@/core/api";
import { toast } from "@/core/toast";

import { followConversion } from "./conversion";

afterEach(() => {
  vi.restoreAllMocks();
  vi.useRealTimers();
});

function job(status: BackgroundJob["status"], result: BackgroundJob["result"] = null): BackgroundJob {
  return {
    id: "conversion-job",
    kind: "library_conversion",
    status,
    parameters: {},
    result,
    error: null,
    progress_current: 0,
    progress_total: null,
    progress_phase: "",
    progress_message: "",
    attempts: 1,
    retry_of_id: null,
    created_at: "2026-10-25T00:00:00Z",
    updated_at: "2026-10-25T00:00:00Z",
    started_at: null,
    finished_at: null,
  };
}

it("waits for the job, refreshes the library and reports what was saved", async () => {
  vi.useFakeTimers();
  const get = vi
    .spyOn(jobsApi, "get")
    .mockResolvedValueOnce(job("running"))
    .mockResolvedValueOnce(
      job("succeeded", { converted: 2, already_light: 1, failed: 0, saved_bytes: 3 * 1024 * 1024 }),
    );
  const success = vi.spyOn(toast, "success").mockImplementation(() => 0);
  const onFinished = vi.fn();

  const following = followConversion("conversion-job", onFinished);
  await vi.advanceTimersByTimeAsync(2_000);
  await following;

  expect(get).toHaveBeenCalledTimes(2);
  expect(onFinished).toHaveBeenCalledOnce();
  expect(success).toHaveBeenCalledWith(
    "Converted 2 tracks",
    "saved 3.0 MB · 1 already light or not convertible",
  );
});

it("names failed tracks instead of reporting success", async () => {
  vi.spyOn(jobsApi, "get").mockResolvedValue(
    job("succeeded", {
      converted: 0,
      failed: 1,
      failures: [{ track_id: 9, reason: "encoded track copy is missing" }],
    }),
  );
  const warn = vi.spyOn(toast, "warn").mockImplementation(() => 0);

  await followConversion("conversion-job", vi.fn());

  expect(warn).toHaveBeenCalledWith(
    "Converted 0, 1 failed",
    "#9: encoded track copy is missing",
  );
});
