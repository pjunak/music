import type { BackgroundJob, BackgroundJobStatus } from "@/core/api";

const ACTIVE_STATUSES = new Set<BackgroundJobStatus>([
  "queued",
  "running",
  "cancel_requested",
]);

export function isBackgroundJobActive(
  job: Pick<BackgroundJob, "status"> | null | undefined,
): boolean {
  return job !== null && job !== undefined && ACTIVE_STATUSES.has(job.status);
}

export function readableBackgroundJobError(
  error: string | null,
  fallback: string,
): string {
  if (error === null) return fallback;
  const match = /^[A-Za-z][A-Za-z0-9_]*Error: (.+)$/s.exec(error);
  return match?.[1] ?? error;
}
