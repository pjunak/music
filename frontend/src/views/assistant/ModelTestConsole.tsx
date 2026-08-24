import type { BackgroundJob } from "@/core/api";
import type {
  ModelConformance,
  ModelQualityEvaluation,
  ModelRole,
  ProviderAdapter,
  ProviderConnection,
} from "@/core/assistantProvidersApi";
import { toast } from "@/core/toast";

import { readableBackgroundJobError } from "./backgroundJobs";
import {
  modelQualityView,
  qualityGateSummary,
  qualityProgressLabel,
  qualityStatusLabel,
  qualityTone,
  taskNameForRole,
  type TestTone,
} from "./modelQualityUi";
import { providerUsageFromJob } from "./modelUsage";
import { modelTestFailureMessage } from "./providerUi";

interface Props {
  open: boolean;
  roles: ModelRole[];
  evaluations: ModelQualityEvaluation[];
  history: BackgroundJob[];
  connections: ProviderConnection[];
  adapters: ProviderAdapter[];
  selectedRoleId: string | null;
  testingRoleId: string | null;
  modelTestResults: Record<string, ModelConformance | undefined>;
  modelTestErrors: Record<string, string | undefined>;
  qualityLoading: boolean;
  qualityLoadError: string | null;
  onSelectRole: (roleId: string) => void;
  onRetryQuality: () => void;
  onRetestFailed: (evaluation: ModelQualityEvaluation) => void;
  qualityActionBusy: boolean;
  onOpenChange: (open: boolean) => void;
}

interface LogEntry {
  id: string;
  time: string | null;
  tone: TestTone;
  message: string;
}

const TONE_LABELS: Record<TestTone, string> = {
  info: "INFO",
  success: "PASS",
  warning: "WARN",
  failure: "FAIL",
  muted: "WAIT",
};

function shortTime(value: string | null): string {
  if (value === null) return "--:--:--";
  const parsed = new Date(value);
  if (Number.isNaN(parsed.getTime())) return "--:--:--";
  return new Intl.DateTimeFormat(undefined, {
    hour: "2-digit",
    minute: "2-digit",
    second: "2-digit",
  }).format(parsed);
}

function count(value: number): string {
  return new Intl.NumberFormat().format(value);
}

function thinkingModeLabel(mode: ModelRole["thinking_mode"]): string {
  return mode === "provider_default" ? "provider default" : mode;
}

function structuredOutputTroubleshooting(
  role: ModelRole,
  failures: string[],
): string | null {
  const outputBudgetFailure = failures.some(
    (failure) =>
      failure.includes("model_execution_empty_structured_output") ||
      failure.includes("model_execution_incomplete_structured_output") ||
      failure.includes("model_output_incomplete"),
  );
  if (!outputBudgetFailure) return null;
  if (role.thinking_mode === "enabled") {
    return (
      "The provider returned no complete final JSON while Thinking was On. " +
      "Turn Thinking Off for this task and rerun; raise the response-token limit " +
      "only when reasoning is genuinely needed."
    );
  }
  if (role.thinking_mode === "provider_default") {
    return (
      "The provider returned no complete final JSON. If it reasons by default, " +
      "choose Thinking Off; otherwise raise the response-token limit before a deliberate rerun."
    );
  }
  return (
    "The provider returned no complete final JSON even with Thinking Off. " +
    "Raise the response-token limit or try a different model before rerunning."
  );
}

function currentModelTestResult(
  role: ModelRole,
  result: ModelConformance | undefined,
): ModelConformance | null {
  return result?.role.connection_id === role.connection_id &&
    result.role.model_id === role.model_id &&
    result.role.last_conformance_at === role.last_conformance_at
    ? result
    : null;
}

function buildLogEntries(
  role: ModelRole,
  evaluation: ModelQualityEvaluation | undefined,
  jobs: BackgroundJob[],
  modelResult: ModelConformance | null,
  modelTestError: string | undefined,
  testing: boolean,
  qualityLoading: boolean,
  qualityLoadError: string | null,
): LogEntry[] {
  const entries: LogEntry[] = [];
  const quality = modelQualityView(evaluation, role, jobs);
  const gateSummary = qualityGateSummary(quality.reportJob);
  entries.push({
    id: "configuration",
    time: role.updated_at,
    tone: role.connection_id === null ? "muted" : "info",
    message:
      role.connection_id === null
        ? "Choose a verified connection and model, then save this task."
        : `Using ${role.connection_name ?? "saved connection"} · ${role.model_id} · thinking ${thinkingModeLabel(role.thinking_mode)}.`,
  });

  if (testing) {
    entries.push({
      id: "model-testing",
      time: null,
      tone: "info",
      message: "Running the one-time model response test…",
    });
  } else if (role.conformance_status === "passed") {
    const responseSummary = modelResult
      ? ` Provider reported ${modelResult.provider_model_id ?? "no model ID"} in ${modelResult.duration_ms} ms; ${modelResult.input_tokens ?? "?"} input and ${modelResult.output_tokens ?? "?"} output tokens.`
      : "";
    entries.push({
      id: "model-passed",
      time: role.last_conformance_at,
      tone: "success",
      message: `Model response test passed.${responseSummary}`,
    });
  } else if (role.conformance_status === "failed") {
    entries.push({
      id: "model-failed",
      time: role.last_conformance_at,
      tone: "failure",
      message: modelTestFailureMessage(role.conformance_error_code),
    });
  } else {
    entries.push({
      id: "model-waiting",
      time: null,
      tone: "muted",
      message: "Model response test has not run for these settings.",
    });
  }

  if (modelTestError !== undefined) {
    entries.push({
      id: "model-request-error",
      time: null,
      tone: "failure",
      message: `The latest model test could not run: ${modelTestError}`,
    });
  }

  if (evaluation !== undefined) {
    entries.push({
      id: "quality-suite",
      time: null,
      tone: "info",
      message: `Quality suite ${evaluation.label} · ${evaluation.suite_id}.`,
    });
  }

  if (
    role.role_id === "music_tagger" &&
    evaluation !== undefined &&
    gateSummary !== null &&
    gateSummary.safetyTotalCases > 0
  ) {
    const totalAttempts =
      evaluation.total_cases + gateSummary.safetyTotalCases;
    entries.push({
      id: "quality-suite-counts",
      time: null,
      tone: "info",
      message:
        `The score covers ${evaluation.total_cases} distinct scenarios. ` +
        `${gateSummary.safetyTotalCases} safety ${gateSummary.safetyTotalCases === 1 ? "scenario runs" : "scenarios run"} twice for stability, so a full suite scores ${totalAttempts} model attempts. Any contract-recovery requests are recorded separately in provider usage.`,
    });
  }

  if (role.conformance_status === "passed" && !role.effective_enabled) {
    entries.push({
      id: "enable-task",
      time: null,
      tone: "warning",
      message:
        "Model test passed. Select “Allow for task” and save before running quality scenarios.",
    });
  }

  if (qualityLoadError !== null) {
    entries.push({
      id: "quality-load-error",
      time: null,
      tone: "failure",
      message: `Stored quality checks could not be loaded: ${qualityLoadError}`,
    });
  } else if (qualityLoading && evaluation === undefined) {
    entries.push({
      id: "quality-loading",
      time: null,
      tone: "info",
      message: "Loading the saved task-quality status…",
    });
  } else if (evaluation === undefined) {
    entries.push({
      id: "quality-unavailable",
      time: null,
      tone: "warning",
      message: "No task-quality suite is registered for this task.",
    });
  } else if (quality.activeJob !== undefined) {
    const active = quality.activeJob;
    entries.push({
      id: `quality-active-${active.id}`,
      time: active.updated_at,
      tone: active.status === "cancel_requested" ? "warning" : "info",
      message:
        active.progress_message ||
        (active.status === "cancel_requested"
          ? "Cancelling the quality check…"
          : "The quality check is running."),
    });
  } else if (evaluation.status === "passed") {
    entries.push({
      id: "quality-passed",
      time: evaluation.last_evaluated_at,
      tone: "success",
      message:
        gateSummary === null
          ? `Task quality passed ${evaluation.passed_cases} of ${evaluation.total_cases} synthetic scenarios.`
          : `Quality gate passed ${evaluation.passed_cases} of ${evaluation.total_cases} scenarios: ${gateSummary.safetyPassedCases} of ${gateSummary.safetyTotalCases} safety checks and ${gateSummary.qualityPassedCases} of ${gateSummary.qualityTotalCases} scored checks (minimum ${Math.round(gateSummary.minimumQualityPassRate * 100)}%).`,
    });
  } else if (evaluation.status === "failed") {
    entries.push({
      id: "quality-failed",
      time: evaluation.last_evaluated_at,
      tone: "failure",
      message:
        gateSummary === null
          ? `Task quality passed ${evaluation.passed_cases} of ${evaluation.total_cases} scenarios. Review the failures below.`
          : `Quality gate failed after ${evaluation.passed_cases} of ${evaluation.total_cases} scenarios: ${gateSummary.safetyPassedCases} of ${gateSummary.safetyTotalCases} safety checks and ${gateSummary.qualityPassedCases} of ${gateSummary.qualityTotalCases} scored checks (minimum ${Math.round(gateSummary.minimumQualityPassRate * 100)}%).`,
    });
  } else if (
    evaluation.status === "stale" ||
    quality.currentJob?.status === "succeeded"
  ) {
    entries.push({
      id: "quality-stale",
      time: evaluation.last_evaluated_at,
      tone: "warning",
      message: "The previous quality result belongs to older model settings.",
    });
  } else if (quality.currentJob?.status === "failed") {
    entries.push({
      id: `quality-interrupted-${quality.currentJob.id}`,
      time: quality.currentJob.finished_at,
      tone: "failure",
      message: readableBackgroundJobError(
        quality.currentJob.error,
        "The quality check did not finish.",
      ),
    });
  } else if (quality.currentJob?.status === "cancelled") {
    entries.push({
      id: `quality-cancelled-${quality.currentJob.id}`,
      time: quality.currentJob.finished_at,
      tone: "warning",
      message: "The quality check was cancelled; no decision was saved.",
    });
  } else {
    entries.push({
      id: "quality-waiting",
      time: null,
      tone: "muted",
      message: `The ${taskNameForRole(role.role_id)} quality suite has not run.`,
    });
  }

  for (const scenario of quality.failures) {
    entries.push({
      id: `scenario-${scenario.id}`,
      time: evaluation?.last_evaluated_at ?? null,
      tone: scenario.blocking ? "failure" : "warning",
      message: `${scenario.description}: ${scenario.failures.join("; ") || "Scenario failed."}`,
    });
    const troubleshooting = structuredOutputTroubleshooting(
      role,
      scenario.failures,
    );
    if (troubleshooting !== null) {
      entries.push({
        id: `scenario-${scenario.id}-troubleshooting`,
        time: evaluation?.last_evaluated_at ?? null,
        tone: "warning",
        message: troubleshooting,
      });
    }
  }

  const usage = providerUsageFromJob(quality.currentJob);
  if (usage !== null) {
    const missing = Math.max(
      usage.attempted_requests - usage.input_tokens_reported_requests,
      usage.attempted_requests - usage.output_tokens_reported_requests,
    );
    const reportedModels =
      usage.provider_model_ids.length > 0
        ? ` · reported model ${usage.provider_model_ids.join(", ")}${
            usage.provider_model_ids_truncated ? " and additional IDs" : ""
          }`
        : "";
    entries.push({
      id: "provider-usage",
      time: quality.currentJob?.finished_at ?? quality.currentJob?.updated_at ?? null,
      tone: missing > 0 ? "warning" : "info",
      message:
        `${count(usage.attempted_requests)} provider calls · ${count(usage.input_tokens)} input tokens · ${count(usage.output_tokens)} output tokens${reportedModels}` +
        (missing > 0 ? ` · ${missing} calls omitted one or both token counts.` : "."),
    });
  }

  if (quality.historicalJob !== undefined) {
    const historicalError = readableBackgroundJobError(
      quality.historicalJob.error,
      "The previous attempt did not finish.",
    );
    entries.push({
      id: `historical-${quality.historicalJob.id}`,
      time: quality.historicalJob.finished_at,
      tone: "warning",
      message:
        `Previous attempt: ${historicalError} ` +
        "It is retained for troubleshooting but does not certify the current task settings.",
    });
  }
  return entries;
}

export function ModelTestConsole({
  open,
  roles,
  evaluations,
  history,
  connections,
  adapters,
  selectedRoleId,
  testingRoleId,
  modelTestResults,
  modelTestErrors,
  qualityLoading,
  qualityLoadError,
  onSelectRole,
  onRetryQuality,
  onRetestFailed,
  qualityActionBusy,
  onOpenChange,
}: Props) {
  const taskRoles = roles.filter((role) => role.configuration_available);
  const role =
    taskRoles.find((item) => item.role_id === selectedRoleId) ?? taskRoles[0];
  const evaluation = evaluations.find((item) => item.role_id === role?.role_id);
  const jobs = role
    ? history.filter(
        (job) =>
          job.parameters.role_id === role.role_id &&
          job.parameters.evaluation_id === evaluation?.evaluation_id,
      )
    : [];
  const quality = modelQualityView(evaluation, role, jobs);
  const connection = connections.find((item) => item.id === role?.connection_id);
  const adapter = adapters.find((item) => item.id === connection?.adapter_id);
  const modelResult = role
    ? currentModelTestResult(role, modelTestResults[role.role_id])
    : null;
  const entries = role
    ? buildLogEntries(
        role,
        evaluation,
        jobs,
        modelResult,
        modelTestErrors[role.role_id],
        testingRoleId === role.role_id,
        qualityLoading,
        qualityLoadError,
      )
    : [];
  const diagnostics = {
    schema_version: "assistant-model-test-console/v1",
    task: role ?? null,
    connection:
      role === undefined
        ? null
        : {
            id: connection?.id ?? role.connection_id,
            name: connection?.name ?? role.connection_name,
            adapter_id: connection?.adapter_id ?? null,
            adapter_label: adapter?.label ?? null,
            base_url: connection?.base_url ?? null,
            verification_status: connection?.verification_status ?? null,
            last_verified_at: connection?.last_verified_at ?? null,
            verified_model_count: connection?.verified_models.length ?? 0,
            verified_capabilities:
              connection?.verified_capability_ids ?? [],
          },
    request:
      role === undefined
        ? null
        : {
            model_id: role.model_id,
            model_was_in_verified_list:
              connection?.verified_models.includes(role.model_id) ?? false,
            timeout_seconds: role.timeout_seconds,
            maximum_response_tokens: role.max_output_tokens,
            thinking_mode: role.thinking_mode,
          },
    model_test: {
      latest_response: modelResult,
      latest_request_error: role ? (modelTestErrors[role.role_id] ?? null) : null,
      running: role?.role_id === testingRoleId,
    },
    quality: {
      evaluation: evaluation ?? null,
      current_job: quality.currentJob ?? null,
      previous_job: quality.historicalJob ?? null,
    },
  };
  const diagnosticsText = JSON.stringify(diagnostics, null, 2);
  const logText = role
    ? [
        `${role.label} test log`,
        `Connection: ${role.connection_name ?? "not configured"}`,
        `Model: ${role.model_id || "not selected"}`,
        `Thinking: ${thinkingModeLabel(role.thinking_mode)}`,
        "",
        ...entries.map(
          (entry) =>
            `[${shortTime(entry.time)}] ${TONE_LABELS[entry.tone].padEnd(4)} ${entry.message}`,
        ),
      ].join("\n")
    : "No configurable model task is available.";
  const consoleStatus =
    role === undefined
      ? "No configurable task"
      : role.conformance_status === "failed"
        ? "Model test failed"
        : qualityStatusLabel(evaluation, quality, qualityLoading);

  async function copy(text: string, kind: "log" | "diagnostics") {
    try {
      await navigator.clipboard.writeText(text);
      toast.success(
        kind === "log" ? "Test log copied" : "Test diagnostics copied",
      );
    } catch {
      toast.error("Copy failed", "Clipboard access was blocked.");
    }
  }

  return (
    <details
      id="assistant-test-console"
      className="surface-card assistant-test-console"
      open={open}
      onToggle={(event) => onOpenChange(event.currentTarget.open)}
      aria-labelledby="assistant-test-console-title"
    >
      <summary className="assistant-test-console-summary">
        <span>
          <strong id="assistant-test-console-title">Test console</strong>
          <small>{role?.label ?? "Model diagnostics"}</small>
        </span>
        <span>{consoleStatus}</span>
      </summary>
      <div className="assistant-test-console-body">
      <header className="assistant-test-console-heading">
        <div>
          <p className="assistant-eyebrow">Saved test activity</p>
          <strong className="assistant-test-console-task-title">
            {role?.label ?? "Model diagnostics"}
          </strong>
          <p>One place for model checks, quality progress, and troubleshooting.</p>
        </div>
        <div className="assistant-test-console-actions">
          {role?.role_id === "music_tagger" &&
          evaluation !== undefined &&
          quality.failures.length > 0 &&
          quality.activeJob === undefined ? (
            <button
              type="button"
              className="btn-ghost"
              disabled={qualityActionBusy}
              onClick={() => onRetestFailed(evaluation)}
            >
              {qualityActionBusy
                ? "Starting…"
                : `Recheck ${quality.failures.length} failed ${quality.failures.length === 1 ? "scenario" : "scenarios"}`}
            </button>
          ) : null}
          {qualityLoadError !== null ? (
            <button type="button" className="btn-ghost" onClick={onRetryQuality}>
              Retry status
            </button>
          ) : null}
          <button
            type="button"
            className="btn-ghost"
            disabled={role === undefined}
            onClick={() => void copy(logText, "log")}
          >
            Copy log
          </button>
          <button
            type="button"
            className="btn-ghost"
            disabled={role === undefined}
            onClick={() => void copy(diagnosticsText, "diagnostics")}
          >
            Copy details
          </button>
        </div>
      </header>

      <nav className="assistant-test-task-tabs" aria-label="Model task test logs">
        {taskRoles.map((item) => {
          const itemEvaluation = evaluations.find(
            (candidate) => candidate.role_id === item.role_id,
          );
          const itemJobs = history.filter(
            (job) =>
              job.parameters.role_id === item.role_id &&
              job.parameters.evaluation_id === itemEvaluation?.evaluation_id,
          );
          const itemQuality = modelQualityView(itemEvaluation, item, itemJobs);
          const tone =
            item.conformance_status === "failed"
              ? "failure"
              : item.conformance_status === "never" &&
                  itemQuality.activeJob === undefined
                ? "muted"
                : qualityTone(itemEvaluation, itemQuality, qualityLoading);
          const statusLabel =
            item.conformance_status === "failed"
              ? "Model failed"
              : item.conformance_status === "never"
                ? "Model test needed"
                : qualityStatusLabel(
                    itemEvaluation,
                    itemQuality,
                    qualityLoading,
                  );
          return (
            <button
              key={item.role_id}
              type="button"
              className={`assistant-test-task-tab is-${tone}`}
              aria-pressed={item.role_id === role?.role_id}
              onClick={() => onSelectRole(item.role_id)}
            >
              <span>{item.label}</span>
              <strong>{statusLabel}</strong>
            </button>
          );
        })}
      </nav>

      {quality.activeJob?.progress_total !== null &&
      quality.activeJob?.progress_total !== undefined ? (
        <progress
          className="assistant-test-console-progress"
          aria-label={qualityProgressLabel(role?.role_id ?? "")}
          value={quality.activeJob.progress_current}
          max={Math.max(1, quality.activeJob.progress_total)}
        />
      ) : quality.activeJob !== undefined ? (
        <progress
          className="assistant-test-console-progress"
          aria-label={qualityProgressLabel(role?.role_id ?? "")}
        />
      ) : null}

      <div className="assistant-test-log" role="log" aria-live="polite">
        {entries.length > 0 ? (
          entries.map((entry) => (
            <div key={entry.id} className={`assistant-test-log-line is-${entry.tone}`}>
              <time dateTime={entry.time ?? undefined}>{shortTime(entry.time)}</time>
              <strong>{TONE_LABELS[entry.tone]}</strong>
              <span>{entry.message}</span>
            </div>
          ))
        ) : (
          <p className="assistant-test-log-empty">
            Configure a model task to start its test log.
          </p>
        )}
      </div>

      <details className="assistant-test-console-details">
        <summary>Technical details</summary>
        <pre aria-label="Selected model task diagnostics JSON">{diagnosticsText}</pre>
      </details>
      </div>
    </details>
  );
}
