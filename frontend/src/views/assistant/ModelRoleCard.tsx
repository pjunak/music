import { useEffect, useState } from "react";

import type { BackgroundJob } from "@/core/api";
import type {
  ModelQualityEvaluation,
  ModelRole,
  ModelRoleUpdate,
  ModelThinkingMode,
  ProviderAdapter,
  ProviderCapability,
  ProviderConnection,
} from "@/core/assistantProvidersApi";

import { ModelPicker } from "./ModelPicker";
import {
  modelQualityView,
  modelTestStatusLabel,
  modelTestTone,
  qualityStatusLabel,
  qualityTone,
} from "./modelQualityUi";
import { modelTestFailureMessage, roleConnection } from "./providerUi";

interface Props {
  role: ModelRole;
  connections: ProviderConnection[];
  adapters: ProviderAdapter[];
  capabilities: ProviderCapability[];
  credentialStorageReady: boolean;
  busy: boolean;
  testing: boolean;
  qualityEvaluation: ModelQualityEvaluation | undefined;
  qualityHistory: BackgroundJob[];
  qualityLoading: boolean;
  qualityActionBusy: boolean;
  onSave: (roleId: string, payload: ModelRoleUpdate) => Promise<void>;
  onTest: (roleId: string) => Promise<void>;
  onStartQuality: (evaluation: ModelQualityEvaluation) => void;
  onCancelQuality: (jobId: string) => void;
  onViewTestLog: () => void;
  onRemove: (roleId: string) => Promise<void>;
}

function includesEveryCapability(
  availableIds: string[] | undefined,
  requiredIds: string[],
): boolean {
  return (
    availableIds !== undefined &&
    requiredIds.every((capabilityId) => availableIds.includes(capabilityId))
  );
}

function roleStateLabel(
  role: ModelRole,
  configured: boolean,
  credentialSaved: boolean,
  capabilitiesSatisfied: boolean,
  qualityEvaluation: ModelQualityEvaluation | undefined,
  qualityActive: boolean,
): string {
  if (!role.configuration_available) return "Planned";
  if (role.effective_enabled) {
    if (qualityActive) return "Checking quality";
    if (qualityEvaluation?.status === "passed") return "Ready";
    if (qualityEvaluation?.status === "failed") return "Quality check failed";
    if (qualityEvaluation?.status === "stale") return "Checks outdated";
    return "Enabled, checks pending";
  }
  if (role.enabled) {
    if (!credentialSaved) return "API key removed";
    if (role.verification_status !== "verified") return "Waiting for verification";
    if (!capabilitiesSatisfied) return "Capability unavailable";
    if (role.conformance_status !== "passed") return "Needs model test";
    return "Stored key unavailable";
  }
  if (!configured) return "Not configured";
  if (role.conformance_status === "passed") return "Tested, switched off";
  if (role.conformance_status === "failed") return "Model test failed";
  return "Configured, not tested";
}

export function ModelRoleCard({
  role,
  connections,
  adapters,
  capabilities,
  credentialStorageReady,
  busy,
  testing,
  qualityEvaluation,
  qualityHistory,
  qualityLoading,
  qualityActionBusy,
  onSave,
  onTest,
  onStartQuality,
  onCancelQuality,
  onViewTestLog,
  onRemove,
}: Props) {
  const [connectionId, setConnectionId] = useState(role.connection_id ?? "");
  const [modelId, setModelId] = useState(role.model_id);
  const [enabled, setEnabled] = useState(role.enabled);
  const [thinkingMode, setThinkingMode] = useState<ModelThinkingMode>(
    role.thinking_mode,
  );
  const [timeoutSeconds, setTimeoutSeconds] = useState(role.timeout_seconds);
  const [maxOutputTokens, setMaxOutputTokens] = useState(role.max_output_tokens);

  useEffect(() => {
    setConnectionId(role.connection_id ?? "");
    setModelId(role.model_id);
    setEnabled(role.enabled);
    setThinkingMode(role.thinking_mode);
    setTimeoutSeconds(role.timeout_seconds);
    setMaxOutputTokens(role.max_output_tokens);
  }, [role]);

  const connection = roleConnection(connections, connectionId);
  const connectionAdapter = adapters.find(
    (adapter) => adapter.id === connection?.adapter_id,
  );
  const adapterSupportsRole = includesEveryCapability(
    connectionAdapter?.capability_ids,
    role.required_capability_ids,
  );
  const verifiedCapabilitiesSatisfied = includesEveryCapability(
    connection?.verified_capability_ids,
    role.required_capability_ids,
  );
  const verifiedModels =
    connection?.verification_status === "verified"
      ? connection.verified_models
      : [];
  const selectedModelAvailable = verifiedModels.includes(modelId);
  const requiredCapabilityLabels = role.required_capability_ids.map(
    (capabilityId) =>
      capabilities.find((capability) => capability.id === capabilityId)?.label ??
      capabilityId,
  );
  const configured = role.connection_id !== null;
  const configurationMatches =
    connectionId === (role.connection_id ?? "") &&
    modelId.trim() === role.model_id &&
    timeoutSeconds === role.timeout_seconds &&
    maxOutputTokens === role.max_output_tokens &&
    thinkingMode === role.thinking_mode;
  const taskDraftMatches = configurationMatches && enabled === role.enabled;
  const canTest =
    credentialStorageReady &&
    configured &&
    configurationMatches &&
    role.configuration_available &&
    connection?.credential_saved === true &&
    connection?.verification_status === "verified" &&
    connection.verified_models.includes(role.model_id) &&
    verifiedCapabilitiesSatisfied;
  const canEnable =
    canTest && role.conformance_status === "passed";
  const quality = modelQualityView(qualityEvaluation, role, qualityHistory);
  const qualityLabel = qualityStatusLabel(
    qualityEvaluation,
    quality,
    qualityLoading,
  );
  const qualityStatusTone = qualityTone(
    qualityEvaluation,
    quality,
    qualityLoading,
  );
  const activeQualityJob = quality.activeJob;
  const qualityActive = activeQualityJob !== undefined;
  const canRunQuality =
    role.effective_enabled &&
    taskDraftMatches &&
    qualityEvaluation !== undefined &&
    !qualityLoading &&
    !qualityActive;
  const actionsBusy = busy || testing || qualityActionBusy || qualityActive;

  async function save(event: React.FormEvent) {
    event.preventDefault();
    if (
      !role.configuration_available ||
      !connectionId ||
      !selectedModelAvailable
    ) {
      return;
    }
    try {
      await onSave(role.role_id, {
        connection_id: connectionId,
        model_id: modelId.trim(),
        enabled,
        thinking_mode: thinkingMode,
        timeout_seconds: timeoutSeconds,
        max_output_tokens: maxOutputTokens,
      });
    } catch {
      // The parent reports the failure and preserves this draft for correction.
    }
  }

  async function testModelAndAllow() {
    onViewTestLog();
    await onTest(role.role_id);
  }

  const stateLabel = roleStateLabel(
    role,
    configured,
    connection?.credential_saved === true,
    verifiedCapabilitiesSatisfied,
    qualityEvaluation,
    qualityActive,
  );

  if (!role.configuration_available) {
    return (
      <article
        id={`assistant-role-${role.role_id}`}
        className="surface-card assistant-role-card is-planned"
      >
        <div className="assistant-role-heading">
          <div>
            <span className="assistant-role-state">{stateLabel}</span>
            <h3>{role.label}</h3>
          </div>
        </div>
        <p>{role.description}</p>
        <div className="assistant-role-planned-note">
          <strong>Not configurable yet</strong>
          <p>
            This task will require: {requiredCapabilityLabels.join(" · ")}. Its
            input, verification, quality, and review contracts must be implemented
            before a model can be assigned.
          </p>
        </div>
        {configured ? (
          <button
            className="btn-ghost"
            type="button"
            disabled={actionsBusy}
            onClick={() => void onRemove(role.role_id)}
          >
            Clear old draft
          </button>
        ) : null}
      </article>
    );
  }

  return (
    <article
      id={`assistant-role-${role.role_id}`}
      className="surface-card assistant-role-card"
    >
      <div className="assistant-role-heading">
        <div>
          <span
            className={`assistant-role-state${
              role.effective_enabled && qualityEvaluation?.status === "passed"
                ? " is-ready"
                : qualityEvaluation?.status === "failed"
                  ? " is-problem"
                  : ""
            }`}
          >
            {stateLabel}
          </span>
          <h3>{role.label}</h3>
        </div>
        <label className="checkbox-row assistant-role-enabled">
          <input
            type="checkbox"
            aria-label="Allow this model for this task"
            checked={enabled}
            disabled={qualityActive || (!enabled && !canEnable)}
            onChange={(event) => setEnabled(event.target.checked)}
          />
          <span>Allow for task</span>
        </label>
      </div>
      <p>{role.description}</p>

      <form onSubmit={(event) => void save(event)}>
        <label className="field">
          <span className="field-label">Connection</span>
          <select
            aria-label="Connection"
            value={connectionId}
            onChange={(event) => {
              const nextConnectionId = event.target.value;
              setConnectionId(nextConnectionId);
              setModelId(
                nextConnectionId === (role.connection_id ?? "")
                  ? role.model_id
                  : "",
              );
              setEnabled(false);
            }}
          >
            <option value="">Choose a connection</option>
            {connections.map((item) => {
              const adapter = adapters.find(
                (candidate) => candidate.id === item.adapter_id,
              );
              const compatible = includesEveryCapability(
                adapter?.capability_ids,
                role.required_capability_ids,
              );
              return (
                <option key={item.id} value={item.id} disabled={!compatible}>
                  {item.name} · {item.credential_saved && item.key_hint
                    ? item.key_hint
                    : "no key saved"}
                  {!compatible
                    ? " · incompatible connection type"
                    : item.credential_saved && item.verification_status === "verified"
                      ? " · verified"
                      : ""}
                </option>
              );
            })}
          </select>
        </label>

        <div className="field">
          <label
            className="field-label assistant-model-label"
            htmlFor={`assistant-model-${role.role_id}`}
          >
            <span>Model</span>
            {verifiedModels.length > 0 ? (
              <small>{verifiedModels.length} verified</small>
            ) : null}
          </label>
          <ModelPicker
            id={`assistant-model-${role.role_id}`}
            value={modelId}
            models={verifiedModels}
            onChange={(nextModelId) => {
              setModelId(nextModelId);
              setEnabled(false);
            }}
          />
        </div>

        <div className="assistant-role-checks" aria-label={`${role.label} checks`}>
          <a
            className={`assistant-role-check is-${modelTestTone(role)}`}
            href="#assistant-test-console"
            onClick={onViewTestLog}
          >
            <span>Model test</span>
            <strong>{modelTestStatusLabel(role)}</strong>
          </a>
          <a
            className={`assistant-role-check is-${qualityStatusTone}`}
            href="#assistant-test-console"
            onClick={onViewTestLog}
          >
            <span>Quality</span>
            <strong>{qualityLabel}</strong>
          </a>
        </div>
        {!canEnable && connectionId ? (
          <p className="field-hint">
            {connection?.credential_saved !== true
              ? "Save an API key on the selected connection before enabling it."
              : !adapterSupportsRole
                ? `Choose a connection that supports ${requiredCapabilityLabels.join(
                    " and ",
                  )}.`
                : connection.verification_status !== "verified"
                ? "Verify the selected connection before enabling it."
                : !verifiedCapabilitiesSatisfied
                  ? `Verification did not confirm ${requiredCapabilityLabels.join(
                      " and ",
                    )} for this connection.`
              : role.conformance_status !== "passed" || !configurationMatches
                ? "Save and pass the model test before enabling this task."
                : "Encrypted credential storage is unavailable."}
          </p>
        ) : null}

        {role.conformance_status === "failed" ? (
          <p className="assistant-provider-problem" role="status">
            {modelTestFailureMessage(role.conformance_error_code)}
          </p>
        ) : null}

        <div
          className="assistant-role-settings"
          role="group"
          aria-label="Request settings"
        >
          <fieldset className="assistant-thinking-mode">
            <legend>Thinking</legend>
            <div className="assistant-thinking-options">
              {(
                [
                  ["provider_default", "Provider default"],
                  ["enabled", "On"],
                  ["disabled", "Off"],
                ] as const
              ).map(([value, label]) => (
                <label key={value}>
                  <input
                    type="radio"
                    name={`assistant-thinking-${role.role_id}`}
                    value={value}
                    checked={thinkingMode === value}
                    disabled={qualityActive}
                    onChange={() => {
                      setThinkingMode(value);
                      setEnabled(false);
                    }}
                  />
                  <span>{label}</span>
                </label>
              ))}
            </div>
          </fieldset>
          <label className="field">
            <span className="field-label">Timeout (seconds)</span>
            <input
              type="number"
              min={5}
              max={300}
              disabled={qualityActive}
              value={timeoutSeconds}
              onChange={(event) => {
                setTimeoutSeconds(Number(event.target.value));
                setEnabled(false);
              }}
            />
          </label>
          <label className="field">
            <span className="field-label">Maximum response tokens</span>
            <input
              type="number"
              min={128}
              max={65536}
              disabled={qualityActive}
              value={maxOutputTokens}
              onChange={(event) => {
                setMaxOutputTokens(Number(event.target.value));
                setEnabled(false);
              }}
            />
          </label>
        </div>

        <div className="assistant-role-actions">
          <button
            className="btn-primary"
            type="submit"
            disabled={
              actionsBusy || !connectionId || !selectedModelAvailable
            }
          >
            {busy ? "Saving…" : "Save task"}
          </button>
          {configured && role.conformance_status !== "passed" ? (
            <button
              className="btn-secondary"
              type="button"
              disabled={busy || testing || qualityActive || !canTest}
              onClick={() => void testModelAndAllow()}
            >
              {testing ? "Testing and allowing…" : "Test model and allow"}
            </button>
          ) : null}
          {configured && role.conformance_status === "passed" ? (
            activeQualityJob !== undefined ? (
              <button
                className="btn-secondary"
                type="button"
                disabled={
                  qualityActionBusy ||
                  activeQualityJob.status === "cancel_requested"
                }
                onClick={() => {
                  onViewTestLog();
                  onCancelQuality(activeQualityJob.id);
                }}
              >
                {activeQualityJob.status === "cancel_requested"
                  ? "Cancelling…"
                  : "Cancel quality check"}
              </button>
            ) : (
              <button
                className="btn-secondary"
                type="button"
                disabled={qualityActionBusy || !canRunQuality}
                onClick={() => {
                  if (qualityEvaluation === undefined) return;
                  onViewTestLog();
                  onStartQuality(qualityEvaluation);
                }}
              >
                {qualityActionBusy
                  ? "Starting…"
                  : qualityEvaluation?.status === "never" &&
                      quality.currentJob === undefined
                    ? "Run quality check"
                    : "Run quality again"}
              </button>
            )
          ) : null}
          {configured ? (
            <button
              className="btn-ghost assistant-role-clear"
              type="button"
              disabled={actionsBusy}
              onClick={() => void onRemove(role.role_id)}
            >
              Clear
            </button>
          ) : null}
        </div>
      </form>
    </article>
  );
}
