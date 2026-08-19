import type { BackgroundJob, BackgroundJobStatus } from "@/core/api";

const ACTIVE_STATUSES = new Set<BackgroundJobStatus>([
  "queued",
  "running",
  "cancel_requested",
]);

export function isAnalysisJobActive(job: BackgroundJob | undefined): boolean {
  return job !== undefined && ACTIVE_STATUSES.has(job.status);
}

export function analysisStatusLabel(status: BackgroundJobStatus): string {
  return status.replace("_", " ");
}

export function formatAnalysisTime(value: string | null): string {
  if (value === null) return "Never";
  return new Intl.DateTimeFormat(undefined, {
    dateStyle: "medium",
    timeStyle: "short",
  }).format(new Date(value));
}
