import { describe, expect, it } from "vitest";

import {
  isBackgroundJobActive,
  readableBackgroundJobError,
} from "./backgroundJobs";

describe("background job UI helpers", () => {
  it.each(["queued", "running", "cancel_requested"] as const)(
    "treats %s as active",
    (status) => {
      expect(isBackgroundJobActive({ status })).toBe(true);
    },
  );

  it.each(["succeeded", "failed", "cancelled"] as const)(
    "treats %s as terminal",
    (status) => {
      expect(isBackgroundJobActive({ status })).toBe(false);
    },
  );

  it("keeps null job state inactive", () => {
    expect(isBackgroundJobActive(null)).toBe(false);
    expect(isBackgroundJobActive(undefined)).toBe(false);
  });

  it("removes an exception type without hiding ordinary provider errors", () => {
    expect(readableBackgroundJobError("ValueError: invalid output", "Retry."))
      .toBe("invalid output");
    expect(readableBackgroundJobError("Provider rejected the call", "Retry."))
      .toBe("Provider rejected the call");
    expect(readableBackgroundJobError(null, "Retry.")).toBe("Retry.");
  });
});
