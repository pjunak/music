import type * as Http from "@/generated/http";

import { api } from "./http";

export type BackgroundJobStatus =
  | "queued"
  | "running"
  | "cancel_requested"
  | "succeeded"
  | "failed"
  | "cancelled";

export type BackgroundJob = Http.BackgroundJobOut;

export const jobsApi = {
  list: (
    params: {
      kind?: string;
      status?: BackgroundJobStatus;
      limit?: number;
    } = {},
  ) => {
    const query = new URLSearchParams();
    if (params.kind !== undefined) query.set("kind", params.kind);
    if (params.status !== undefined) query.set("status", params.status);
    if (params.limit !== undefined) query.set("limit", String(params.limit));
    const suffix = query.size > 0 ? `?${query.toString()}` : "";
    return api.get<BackgroundJob[]>(`/api/jobs${suffix}`);
  },
  get: (jobId: string) =>
    api.get<BackgroundJob>(`/api/jobs/${encodeURIComponent(jobId)}`),
  cancel: (jobId: string) =>
    api.post<BackgroundJob>(`/api/jobs/${encodeURIComponent(jobId)}/cancel`),
  retry: (jobId: string) =>
    api.post<BackgroundJob>(`/api/jobs/${encodeURIComponent(jobId)}/retry`),
};
