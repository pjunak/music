import type { BackgroundJob } from "@/core/api";
import type {
  ModelQualityEvaluation,
  ModelRole,
} from "@/core/assistantProvidersApi";

import { isModelEvaluationJobActive } from "./modelEvaluationJobs";

export type TestTone = "info" | "success" | "warning" | "failure" | "muted";

export interface FailedScenario {
  id: string;
  description: string;
  blocking: boolean;
  failures: string[];
}

export interface QualityGateSummary {
  safetyPassedCases: number;
  safetyTotalCases: number;
  qualityPassedCases: number;
  qualityTotalCases: number;
  minimumQualityPassRate: number;
}

export interface ModelQualityView {
  latestHistory: BackgroundJob | undefined;
  activeJob: BackgroundJob | undefined;
  reportJob: BackgroundJob | undefined;
  currentJob: BackgroundJob | undefined;
  historicalJob: BackgroundJob | undefined;
  failures: FailedScenario[];
}

function isRecord(value: unknown): value is Record<string, unknown> {
  return typeof value === "object" && value !== null && !Array.isArray(value);
}

export interface QualityEvidenceNote {
  id: string;
  message: string;
  tone: TestTone;
}

function isCount(value: unknown): value is number {
  return typeof value === "number" && Number.isSafeInteger(value) && value >= 0;
}

export function qualityEvidenceNotes(job: BackgroundJob | undefined): QualityEvidenceNote[] {
  const evaluation = job?.result?.evaluation;
  if (!isRecord(evaluation)) return [];
  const notes: QualityEvidenceNote[] = [];
  if (Array.isArray(evaluation.vocabulary_results)) {
    const labels: Record<string, string> = {
      default: "Bundled vocabulary", custom: "Custom vocabulary", maximum: "200-tag vocabulary",
    };
    for (const group of evaluation.vocabulary_results.slice(0, 3)) {
      if (!isRecord(group) || typeof group.vocabulary !== "string" ||
          !Object.hasOwn(labels, group.vocabulary) || typeof group.passed !== "boolean" ||
          !isCount(group.passed_cases) || !isCount(group.total_cases) ||
          group.total_cases === 0 || group.passed_cases > group.total_cases) continue;
      notes.push({
        id: `vocabulary-${group.vocabulary}`,
        tone: group.passed ? "success" : "failure",
        message: `${labels[group.vocabulary]}: ${group.passed_cases}/${group.total_cases} scenarios; ${group.passed ? "passed" : "failed"} its independent quality gate.`,
      });
    }
  }
  if (Array.isArray(evaluation.cases)) {
    for (const scenario of evaluation.cases.slice(0, 100)) {
      if (!isRecord(scenario) || typeof scenario.id !== "string" ||
          typeof scenario.description !== "string" || !isRecord(scenario.candidate_recall)) continue;
      const recall = scenario.candidate_recall;
      if (!isCount(recall.pool_tracks) || recall.pool_tracks > 100 ||
          !isCount(recall.relevant_tracks) || !isCount(recall.relevant_in_pool) ||
          recall.relevant_in_pool > recall.pool_tracks ||
          recall.relevant_in_pool >= recall.relevant_tracks) continue;
      notes.push({
        id: `candidate-pool-${scenario.id}`,
        tone: "warning",
        message: `${scenario.description}: local candidate preparation supplied ${recall.relevant_in_pool}/${recall.relevant_tracks} relevant tracks in a pool of ${recall.pool_tracks}. The model cannot rank tracks absent from its input.`,
      });
    }
  }
  return notes;
}

function failedScenarios(job: BackgroundJob | undefined): FailedScenario[] {
  const evaluation = job?.result?.evaluation;
  if (!isRecord(evaluation) || !Array.isArray(evaluation.cases)) return [];
  return evaluation.cases.flatMap((value) => {
    if (!isRecord(value) || value.passed !== false) return [];
    const failures = Array.isArray(value.failures)
      ? value.failures.filter((item): item is string => typeof item === "string")
      : [];
    return typeof value.id === "string" && typeof value.description === "string"
      ? [{
          id: value.id,
          description: value.description,
          blocking: value.blocking !== false,
          failures,
        }]
      : [];
  });
}

export function qualityGateSummary(
  job: BackgroundJob | undefined,
): QualityGateSummary | null {
  const evaluation = job?.result?.evaluation;
  if (!isRecord(evaluation)) return null;
  const values = [
    evaluation.safety_passed_cases,
    evaluation.safety_total_cases,
    evaluation.quality_passed_cases,
    evaluation.quality_total_cases,
    evaluation.minimum_quality_pass_rate,
  ];
  if (!values.every((value) => typeof value === "number")) return null;
  return {
    safetyPassedCases: evaluation.safety_passed_cases as number,
    safetyTotalCases: evaluation.safety_total_cases as number,
    qualityPassedCases: evaluation.quality_passed_cases as number,
    qualityTotalCases: evaluation.quality_total_cases as number,
    minimumQualityPassRate: evaluation.minimum_quality_pass_rate as number,
  };
}

function attemptFollowsCurrentModelTest(
  job: BackgroundJob,
  role: ModelRole | undefined,
): boolean {
  if (role?.effective_enabled !== true || role.last_conformance_at === null) {
    return false;
  }
  const jobCreatedAt = Date.parse(job.created_at);
  const modelTestedAt = Date.parse(role.last_conformance_at);
  return (
    Number.isFinite(jobCreatedAt) &&
    Number.isFinite(modelTestedAt) &&
    jobCreatedAt >= modelTestedAt
  );
}

export function modelQualityView(
  evaluation: ModelQualityEvaluation | undefined,
  role: ModelRole | undefined,
  history: BackgroundJob[],
): ModelQualityView {
  const latestHistory = history[0];
  const activeJob = isModelEvaluationJobActive(latestHistory)
    ? latestHistory
    : undefined;
  const reportJob =
    evaluation?.last_job_id === null || evaluation === undefined
      ? undefined
      : history.find((job) => job.id === evaluation.last_job_id);
  const currentTerminalAttempt =
    evaluation?.status === "never" &&
    latestHistory !== undefined &&
    !isModelEvaluationJobActive(latestHistory) &&
    attemptFollowsCurrentModelTest(latestHistory, role)
      ? latestHistory
      : undefined;
  const currentJob = activeJob ?? reportJob ?? currentTerminalAttempt;
  const historicalJob =
    latestHistory !== undefined && latestHistory.id !== currentJob?.id
      ? latestHistory
      : undefined;
  return {
    latestHistory,
    activeJob,
    reportJob,
    currentJob,
    historicalJob,
    failures: failedScenarios(reportJob),
  };
}

export function qualityStatusLabel(
  evaluation: ModelQualityEvaluation | undefined,
  view: ModelQualityView,
  loading: boolean,
): string {
  if (loading && evaluation === undefined) return "Loading";
  if (evaluation === undefined) return "Unavailable";
  if (view.activeJob !== undefined) {
    if (view.activeJob.status === "cancel_requested") return "Cancelling";
    if (view.activeJob.progress_total !== null) {
      const progressUnit =
        evaluation.role_id === "music_tagger" ? "scored attempts" : "scenarios";
      return `${view.activeJob.progress_current} / ${view.activeJob.progress_total} ${progressUnit}`;
    }
    return view.activeJob.status === "queued" ? "Queued" : "Running";
  }
  if (evaluation.status === "passed" || evaluation.status === "failed") {
    return evaluation.total_cases > 0
      ? `${evaluation.passed_cases} / ${evaluation.total_cases} scenarios`
      : evaluation.status === "passed"
        ? "Passed"
        : "Failed";
  }
  if (evaluation.status === "stale" || view.currentJob?.status === "succeeded") {
    return "Outdated";
  }
  if (view.currentJob?.status === "failed") return "Interrupted";
  if (view.currentJob?.status === "cancelled") return "Cancelled";
  return "Not run";
}

export function qualityTone(
  evaluation: ModelQualityEvaluation | undefined,
  view: ModelQualityView,
  loading: boolean,
): TestTone {
  if (loading && evaluation === undefined) return "info";
  if (view.activeJob !== undefined) {
    return view.activeJob.status === "cancel_requested" ? "warning" : "info";
  }
  if (evaluation?.status === "passed") return "success";
  if (evaluation?.status === "failed" || view.currentJob?.status === "failed") {
    return "failure";
  }
  if (
    evaluation?.status === "stale" ||
    view.currentJob?.status === "succeeded" ||
    view.currentJob?.status === "cancelled"
  ) {
    return "warning";
  }
  return "muted";
}

export function modelTestTone(role: ModelRole): TestTone {
  if (role.conformance_status === "passed") return "success";
  if (role.conformance_status === "failed") return "failure";
  return "muted";
}

export function modelTestStatusLabel(role: ModelRole): string {
  if (role.conformance_status === "passed") return "Passed";
  if (role.conformance_status === "failed") return "Failed";
  return "Not run";
}

export function taskNameForRole(roleId: string): string {
  if (roleId === "music_tagger") return "mood tagging";
  if (roleId === "tag_cleanup") return "mood-tag cleanup";
  if (roleId === "eq_assistant") return "EQ drafting";
  return "playlist planning";
}

export function qualityProgressLabel(roleId: string): string {
  if (roleId === "music_tagger") return "Mood tagging model quality progress";
  if (roleId === "tag_cleanup") return "Mood-tag cleanup model quality progress";
  if (roleId === "eq_assistant") return "EQ model quality progress";
  return "Playlist model quality progress";
}
