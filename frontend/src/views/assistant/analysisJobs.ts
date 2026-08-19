import type { BackgroundJob, BackgroundJobStatus } from "@/core/api";

import { isBackgroundJobActive } from "./backgroundJobs";

export function isAnalysisJobActive(job: BackgroundJob | undefined): boolean {
  return isBackgroundJobActive(job);
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
