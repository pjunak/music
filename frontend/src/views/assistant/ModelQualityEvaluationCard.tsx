import type { BackgroundJob, BackgroundJobStatus } from "@/core/api";
import type {
  ModelQualityEvaluation,
  ModelRole,
} from "@/core/assistantProvidersApi";

import { readableBackgroundJobError } from "./backgroundJobs";
import { isModelEvaluationJobActive } from "./modelEvaluationJobs";
import { ModelUsageSummary } from "./ModelUsageSummary";
import { TestResultReport } from "./TestResultReport";

interface FailedScenario {
  id: string;
  description: string;
  failures: string[];
}

function isRecord(value: unknown): value is Record<string, unknown> {
  return typeof value === "object" && value !== null && !Array.isArray(value);
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
      ? [{ id: value.id, description: value.description, failures }]
      : [];
  });
}

function formatTime(value: string | null): string {
  if (value === null) return "Never";
  return new Intl.DateTimeFormat(undefined, {
    dateStyle: "medium",
    timeStyle: "short",
  }).format(new Date(value));
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

function previousAttemptLabel(job: BackgroundJob): string {
  if (job.status === "failed") return "Show previous interrupted attempt";
  if (job.status === "cancelled") return "Show previous cancelled attempt";
  return "Show previous attempt";
}

function qualityTestReport(
  evaluation: ModelQualityEvaluation,
  role: ModelRole | undefined,
  job: BackgroundJob | undefined,
): object {
  return {
    schema_version: "assistant-model-quality-report/v1",
    status: evaluation.status,
    evaluation: {
      id: evaluation.evaluation_id,
      label: evaluation.label,
      suite_id: evaluation.suite_id,
      passed_cases: evaluation.passed_cases,
      total_cases: evaluation.total_cases,
      last_evaluated_at: evaluation.last_evaluated_at,
    },
    task: {
      id: role?.role_id ?? evaluation.role_id,
      label: role?.label ?? null,
      connection_id: role?.connection_id ?? null,
      connection_name: role?.connection_name ?? null,
      model_id: role?.model_id ?? null,
      conformance_status: role?.conformance_status ?? null,
      last_conformance_at: role?.last_conformance_at ?? null,
    },
    job:
      job === undefined
        ? null
        : {
            id: job.id,
            kind: job.kind,
            status: job.status,
            attempts: job.attempts,
            created_at: job.created_at,
            started_at: job.started_at,
            finished_at: job.finished_at,
            progress: {
              phase: job.progress_phase,
              message: job.progress_message,
              current: job.progress_current,
              total: job.progress_total,
            },
            error:
              job.error === null
                ? null
                : readableBackgroundJobError(
                    job.error,
                    "The quality check did not finish.",
                  ),
          },
    result: job?.result ?? null,
  };
}

function statusLabel(
  evaluation: ModelQualityEvaluation,
  latest: BackgroundJob | undefined,
): string {
  if (latest !== undefined && isModelEvaluationJobActive(latest)) {
    return latest.status === "cancel_requested" ? "Cancelling" : "Running";
  }
  if (evaluation.status === "passed") return "Passed";
  if (evaluation.status === "failed") return "Needs review";
  if (evaluation.status === "stale") return "Settings changed";
  if (latest?.status === "failed") return "Run interrupted";
  if (latest?.status === "cancelled") return "Cancelled";
  if (latest?.status === "succeeded") return "Needs new check";
  return "Not run";
}

function statusClass(
  evaluation: ModelQualityEvaluation,
  latest: BackgroundJob | undefined,
): BackgroundJobStatus {
  if (latest !== undefined && isModelEvaluationJobActive(latest)) {
    return latest.status;
  }
  if (evaluation.status === "passed") return "succeeded";
  if (evaluation.status === "failed") return "failed";
  if (latest?.status === "failed" || latest?.status === "cancelled") {
    return latest.status;
  }
  return "queued";
}

interface Props {
  evaluation: ModelQualityEvaluation;
  role: ModelRole | undefined;
  history: BackgroundJob[];
  loading: boolean;
  actionBusy: boolean;
  onStart: () => void;
  onCancel: (jobId: string) => void;
}

export function ModelQualityEvaluationCard({
  evaluation,
  role,
  history,
  loading,
  actionBusy,
  onStart,
  onCancel,
}: Props) {
  const taskName =
    evaluation.role_id === "music_tagger"
      ? "music tagging"
      : evaluation.role_id === "tag_cleanup"
        ? "song-tag cleanup"
        : evaluation.role_id === "eq_assistant"
          ? "EQ drafting"
          : "playlist planning";
  const progressLabel =
    evaluation.role_id === "music_tagger"
      ? "Music tagging model quality progress"
      : evaluation.role_id === "tag_cleanup"
        ? "Song-tag cleanup model quality progress"
        : evaluation.role_id === "eq_assistant"
          ? "EQ model quality progress"
          : "Playlist model quality progress";
  const latestHistory = history[0];
  const activeJob = isModelEvaluationJobActive(latestHistory)
    ? latestHistory
    : undefined;
  const reportJob =
    evaluation.last_job_id === null
      ? undefined
      : history.find((job) => job.id === evaluation.last_job_id);
  const currentTerminalAttempt =
    evaluation.status === "never" &&
    latestHistory !== undefined &&
    !isModelEvaluationJobActive(latestHistory) &&
    attemptFollowsCurrentModelTest(latestHistory, role)
      ? latestHistory
      : undefined;
  const currentJob = activeJob ?? reportJob ?? currentTerminalAttempt;
  const active = activeJob !== undefined;
  const historicalJob =
    latestHistory !== undefined && latestHistory.id !== currentJob?.id
      ? latestHistory
      : undefined;
  const failures = failedScenarios(reportJob);
  const canRun = role?.effective_enabled === true;

  return (
    <article className="surface-card assistant-quality-card">
      <div className="assistant-section-heading">
        <div>
          <p className="assistant-eyebrow">Synthetic provider check</p>
          <h3>{evaluation.label}</h3>
        </div>
        <span
          className={`assistant-job-status is-${statusClass(evaluation, currentJob)}`}
        >
          {statusLabel(evaluation, currentJob)}
        </span>
      </div>
      <p>{evaluation.description}</p>

      {loading && currentJob === undefined ? (
        <p className="muted">Loading stored evaluation history…</p>
      ) : active && activeJob !== undefined ? (
        <div className="assistant-job-progress">
          <div className="assistant-job-progress-label">
            <strong>{activeJob.progress_phase || "Queued"}</strong>
            {activeJob.progress_total !== null ? (
              <span>
                {activeJob.progress_current} / {activeJob.progress_total}
              </span>
            ) : null}
          </div>
          {activeJob.progress_total === null ? (
            <progress aria-label={progressLabel} />
          ) : (
            <progress
              aria-label={progressLabel}
              value={activeJob.progress_current}
              max={Math.max(1, activeJob.progress_total)}
            />
          )}
          {activeJob.progress_message ? <p>{activeJob.progress_message}</p> : null}
        </div>
      ) : evaluation.status === "passed" ? (
        <div className="assistant-quality-result">
          <strong>
            Passed {evaluation.passed_cases} of {evaluation.total_cases} scenarios
          </strong>
          <p>
            This model passed the current synthetic {taskName} gate for these exact
            settings.
          </p>
        </div>
      ) : evaluation.status === "failed" ? (
        <div className="assistant-quality-result is-failed">
          <strong>
            Passed {evaluation.passed_cases} of {evaluation.total_cases} scenarios
          </strong>
          <p>
            Local tools remain available. Review the failed scenarios before trying
            this model again; it cannot be used for {taskName} yet.
          </p>
        </div>
      ) : evaluation.status === "stale" || currentJob?.status === "succeeded" ? (
        <div className="assistant-quality-result">
          <strong>These model settings need a new check</strong>
          <p>The previous report is history only and no longer certifies this role.</p>
        </div>
      ) : currentJob?.status === "failed" ? (
        <div className="assistant-quality-result is-failed">
          <strong>The evaluation did not finish</strong>
          <p>
            {readableBackgroundJobError(
              currentJob.error,
              "Run the check again when the provider is available.",
            )}
          </p>
        </div>
      ) : currentJob?.status === "cancelled" ? (
        <div className="assistant-quality-result">
          <strong>The evaluation was cancelled</strong>
          <p>No quality decision was saved.</p>
        </div>
      ) : (
        <div className="assistant-quality-result">
          <strong>No quality report yet</strong>
          <p>
            Enable the tested {taskName} model, then run its fixed synthetic suite.
          </p>
        </div>
      )}

      {failures.length > 0 ? (
        <details className="assistant-job-failures assistant-quality-failures">
          <summary>
            Review {failures.length} failed{" "}
            {failures.length === 1 ? "scenario" : "scenarios"}
          </summary>
          <ul>
            {failures.map((scenario) => (
              <li key={scenario.id}>
                <strong>{scenario.description}</strong>
                {scenario.failures.map((failure, index) => (
                  <span key={`${index}:${failure}`}>{failure}</span>
                ))}
              </li>
            ))}
          </ul>
        </details>
      ) : null}

      <ModelUsageSummary job={currentJob} />

      {currentJob !== undefined || evaluation.status !== "never" ? (
        <TestResultReport
          label={`${evaluation.label} result`}
          report={qualityTestReport(evaluation, role, currentJob)}
          openByDefault={
            evaluation.status === "failed" || currentJob?.status === "failed"
          }
        />
      ) : null}

      {historicalJob !== undefined ? (
        <details className="assistant-quality-history">
          <summary>{previousAttemptLabel(historicalJob)}</summary>
          <div>
            {historicalJob.status === "failed" ? (
              <p>
                {readableBackgroundJobError(
                  historicalJob.error,
                  "The previous evaluation did not finish.",
                )}
              </p>
            ) : historicalJob.status === "cancelled" ? (
              <p>The previous evaluation was cancelled.</p>
            ) : (
              <p>This attempt is retained as history and does not certify the current role.</p>
            )}
            <ModelUsageSummary job={historicalJob} />
          </div>
        </details>
      ) : null}

      <div className="assistant-quality-meta">
        <span>Suite: {evaluation.suite_id}</span>
        <span>Current result: {formatTime(evaluation.last_evaluated_at)}</span>
      </div>
      <p className="assistant-quality-disclosure">
        The provider receives only fixed synthetic scenarios. It receives no songs,
        audio, filesystem paths, or live library data. Determinism checks can make
        more than one model call and may incur provider cost.
      </p>

      <div className="assistant-role-actions">
        {active && activeJob !== undefined ? (
          <button
            type="button"
            className="btn-secondary"
            disabled={actionBusy || activeJob.status === "cancel_requested"}
            onClick={() => onCancel(activeJob.id)}
          >
            {activeJob.status === "cancel_requested"
              ? "Cancelling…"
              : "Cancel check"}
          </button>
        ) : (
          <button
            type="button"
            className="btn-primary"
            disabled={actionBusy || !canRun}
            onClick={onStart}
          >
            {actionBusy
              ? "Starting…"
              : evaluation.status === "never" && currentJob === undefined
                ? "Run quality check"
                : "Run quality check again"}
          </button>
        )}
      </div>
      {!canRun ? (
        <p className="field-hint">
          <a href={`#assistant-role-${evaluation.role_id}`}>
            Review the {taskName} model task above
          </a>{" "}
          to save, test, and enable it before running this check.
        </p>
      ) : null}
    </article>
  );
}
