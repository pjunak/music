import type { BackgroundJob, EqPresetDraft } from "@/core/api";

import { isBackgroundJobActive } from "./backgroundJobs";

export const MODEL_EQ_DRAFT_JOB_KIND = "assistant.model-eq-draft";

function isRecord(value: unknown): value is Record<string, unknown> {
  return typeof value === "object" && value !== null && !Array.isArray(value);
}

export function isEqDraftJobActive(
  job: BackgroundJob | null | undefined,
): boolean {
  return isBackgroundJobActive(job);
}

export function eqDraftFromJob(
  job: BackgroundJob | null | undefined,
): EqPresetDraft | null {
  if (job?.status !== "succeeded" || !isRecord(job.result)) return null;
  if (job.result.schema_version !== "assistant-eq-draft-job-result/v1") return null;
  const draft = job.result.draft;
  if (!isRecord(draft)) return null;
  if (
    typeof draft.name !== "string" ||
    typeof draft.goal !== "string" ||
    typeof draft.rationale !== "string" ||
    !Array.isArray(draft.cautions) ||
    !draft.cautions.every((item) => typeof item === "string") ||
    !Array.isArray(draft.bands) ||
    draft.bands.length !== 10
  ) {
    return null;
  }
  const bands = draft.bands.flatMap((value) => {
    if (
      !isRecord(value) ||
      typeof value.frequency !== "number" ||
      typeof value.gain !== "number" ||
      !Number.isFinite(value.frequency) ||
      !Number.isFinite(value.gain)
    ) {
      return [];
    }
    return [{ frequency: value.frequency, gain: value.gain }];
  });
  if (bands.length !== 10) return null;
  return {
    name: draft.name,
    goal: draft.goal,
    bands,
    rationale: draft.rationale,
    cautions: [...draft.cautions],
  };
}
