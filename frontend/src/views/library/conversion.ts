import { jobsApi } from "@/core/api";
import { toast } from "@/core/toast";

import { formatSize } from "./shared";

const POLL_INTERVAL_MS = 2_000;
const ACTIVE = new Set(["queued", "running", "cancel_requested"]);

interface ConversionResult {
  converted?: number;
  already_light?: number;
  unsupported?: number;
  failed?: number;
  saved_bytes?: number;
  failures?: { track_id: number; reason: string }[];
}

/** Follows a conversion job to its end, reports what it did and calls
 *  `onFinished` so the library shows the converted files. */
export async function followConversion(
  jobId: string,
  onFinished: () => void,
): Promise<void> {
  let current;
  try {
    current = await jobsApi.get(jobId);
    while (ACTIVE.has(current.status)) {
      await new Promise((resolve) => window.setTimeout(resolve, POLL_INTERVAL_MS));
      current = await jobsApi.get(current.id);
    }
  } catch (e) {
    toast.warn(
      "Lost track of the conversion",
      e instanceof Error ? e.message : "It keeps running on the server.",
    );
    return;
  }
  onFinished();
  if (current.status !== "succeeded") {
    toast.error("Conversion stopped", current.error ?? `The job ${current.status}.`);
    return;
  }
  const result = (current.result ?? {}) as ConversionResult;
  const converted = result.converted ?? 0;
  const kept = (result.already_light ?? 0) + (result.unsupported ?? 0);
  const parts = [
    converted > 0 ? `saved ${formatSize(Math.max(0, result.saved_bytes ?? 0))}` : "",
    kept > 0 ? `${kept} already light or not convertible` : "",
  ].filter(Boolean);
  const failed = result.failed ?? 0;
  if (failed > 0) {
    const sample = (result.failures ?? [])
      .slice(0, 3)
      .map((failure) => `#${failure.track_id}: ${failure.reason}`)
      .join("\n");
    toast.warn(`Converted ${converted}, ${failed} failed`, [parts.join(" · "), sample].filter(Boolean).join("\n"));
  } else {
    toast.success(
      `Converted ${converted} track${converted === 1 ? "" : "s"}`,
      parts.join(" · ") || undefined,
    );
  }
}
