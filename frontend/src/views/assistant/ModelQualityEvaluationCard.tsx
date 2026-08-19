import type { BackgroundJob, BackgroundJobStatus } from "@/core/api";
import type {
  ModelQualityEvaluation,
  ModelRole,
} from "@/core/assistantProvidersApi";

import { readableBackgroundJobError } from "./backgroundJobs";
import { isModelEvaluationJobActive } from "./modelEvaluationJobs";
import { ModelUsageSummary } from "./ModelUsageSummary";

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
  const isMusicTagging = evaluation.role_id === "music_tagger";
  const taskName = isMusicTagging ? "music tagging" : "playlist planning";
  const progressLabel = isMusicTagging
    ? "Music tagging model quality progress"
    : "Playlist model quality progress";
  const latest = history[0];
  const active = isModelEvaluationJobActive(latest);
  const reportJob =
    latest !== undefined && evaluation.last_job_id === latest.id ? latest : undefined;
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
          className={`assistant-job-status is-${statusClass(evaluation, latest)}`}
        >
          {statusLabel(evaluation, latest)}
        </span>
      </div>
      <p>{evaluation.description}</p>

      {loading && latest === undefined ? (
        <p className="muted">Loading stored evaluation history…</p>
      ) : active && latest !== undefined ? (
        <div className="assistant-job-progress">
          <div className="assistant-job-progress-label">
            <strong>{latest.progress_phase || "Queued"}</strong>
            {latest.progress_total !== null ? (
              <span>
                {latest.progress_current} / {latest.progress_total}
              </span>
            ) : null}
          </div>
          {latest.progress_total === null ? (
            <progress aria-label={progressLabel} />
          ) : (
            <progress
              aria-label={progressLabel}
              value={latest.progress_current}
              max={Math.max(1, latest.progress_total)}
            />
          )}
          {latest.progress_message ? <p>{latest.progress_message}</p> : null}
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
      ) : evaluation.status === "stale" || latest?.status === "succeeded" ? (
        <div className="assistant-quality-result">
          <strong>These model settings need a new check</strong>
          <p>The previous report is history only and no longer certifies this role.</p>
        </div>
      ) : latest?.status === "failed" ? (
        <div className="assistant-quality-result is-failed">
          <strong>The evaluation did not finish</strong>
          <p>
            {readableBackgroundJobError(
              latest.error,
              "Run the check again when the provider is available.",
            )}
          </p>
        </div>
      ) : latest?.status === "cancelled" ? (
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

      <ModelUsageSummary job={reportJob} />

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
        {active && latest !== undefined ? (
          <button
            type="button"
            className="btn-secondary"
            disabled={actionBusy || latest.status === "cancel_requested"}
            onClick={() => onCancel(latest.id)}
          >
            {latest.status === "cancel_requested" ? "Cancelling…" : "Cancel check"}
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
              : evaluation.status === "never" && latest === undefined
                ? "Run quality check"
                : "Run quality check again"}
          </button>
        )}
      </div>
      {!canRun ? (
        <p className="field-hint">
          Save, test, and enable the {taskName} role before running this check.
        </p>
      ) : null}
    </article>
  );
}
