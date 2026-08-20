import type { BackgroundJob, EqPresetDraft } from "@/core/api";
import {
  EQ_FREQUENCIES,
  EQ_GAIN_MAX,
  EQ_GAIN_MIN,
  EQ_GAIN_STEP,
} from "@/core/eq";

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
  const bands: EqPresetDraft["bands"] = [];
  for (const [index, value] of draft.bands.entries()) {
    if (
      !isRecord(value) ||
      typeof value.frequency !== "number" ||
      typeof value.gain !== "number" ||
      !Number.isFinite(value.frequency) ||
      !Number.isFinite(value.gain) ||
      value.frequency !== EQ_FREQUENCIES[index] ||
      value.gain < EQ_GAIN_MIN ||
      value.gain > EQ_GAIN_MAX ||
      Math.abs(value.gain / EQ_GAIN_STEP - Math.round(value.gain / EQ_GAIN_STEP)) >
        Number.EPSILON
    ) {
      return null;
    }
    bands.push({ frequency: value.frequency, gain: value.gain });
  }
  return {
    name: draft.name,
    goal: draft.goal,
    bands,
    rationale: draft.rationale,
    cautions: [...draft.cautions],
  };
}
