import { useEffect, useState } from "react";

import type {
  ModelConformance,
  ModelRole,
  ModelRoleUpdate,
  ProviderAdapter,
  ProviderCapability,
  ProviderConnection,
} from "@/core/assistantProvidersApi";

import { ModelPicker } from "./ModelPicker";
import { modelTestFailureMessage, roleConnection } from "./providerUi";
import { TestResultReport } from "./TestResultReport";

interface Props {
  role: ModelRole;
  connections: ProviderConnection[];
  adapters: ProviderAdapter[];
  capabilities: ProviderCapability[];
  credentialStorageReady: boolean;
  busy: boolean;
  testing: boolean;
  onSave: (roleId: string, payload: ModelRoleUpdate) => Promise<void>;
  onTest: (roleId: string) => Promise<ModelConformance | null>;
  onRemove: (roleId: string) => Promise<void>;
}

function modelTestReport(
  role: ModelRole,
  connection: ProviderConnection | undefined,
  adapter: ProviderAdapter | undefined,
  result: ModelConformance | null,
): object {
  const errorCode = role.conformance_error_code;
  return {
    schema_version: "assistant-model-test-report/v1",
    status: role.conformance_status,
    tested_at: role.last_conformance_at,
    task: {
      id: role.role_id,
      label: role.label,
      required_capabilities: role.required_capability_ids,
    },
    connection: {
      id: connection?.id ?? role.connection_id,
      name: connection?.name ?? role.connection_name,
      adapter_id: connection?.adapter_id ?? null,
      adapter_label: adapter?.label ?? null,
      base_url: connection?.base_url ?? null,
      verification_status: connection?.verification_status ?? null,
      last_verified_at: connection?.last_verified_at ?? null,
      verified_model_count: connection?.verified_models.length ?? 0,
      verified_capabilities: connection?.verified_capability_ids ?? [],
    },
    request: {
      model_id: role.model_id,
      model_was_in_verified_list:
        connection?.verified_models.includes(role.model_id) ?? false,
      timeout_seconds: role.timeout_seconds,
      maximum_response_tokens: role.max_output_tokens,
    },
    provider_response: {
      contract_version:
        result?.contract_version ?? "assistant-provider-conformance/v3",
      reported_model_id: result?.provider_model_id ?? null,
      finish_reason: result?.finish_reason ?? null,
      input_tokens: result?.input_tokens ?? null,
      output_tokens: result?.output_tokens ?? null,
      duration_ms: result?.duration_ms ?? null,
    },
    error:
      errorCode === null
        ? null
        : {
            code: errorCode,
            message: modelTestFailureMessage(errorCode),
          },
  };
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
): string {
  if (!role.configuration_available) return "Planned";
  if (role.effective_enabled) return "Enabled";
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
  onSave,
  onTest,
  onRemove,
}: Props) {
  const [connectionId, setConnectionId] = useState(role.connection_id ?? "");
  const [modelId, setModelId] = useState(role.model_id);
  const [enabled, setEnabled] = useState(role.enabled);
  const [timeoutSeconds, setTimeoutSeconds] = useState(role.timeout_seconds);
  const [maxOutputTokens, setMaxOutputTokens] = useState(role.max_output_tokens);
  const [lastTestResult, setLastTestResult] =
    useState<ModelConformance | null>(null);

  useEffect(() => {
    setConnectionId(role.connection_id ?? "");
    setModelId(role.model_id);
    setEnabled(role.enabled);
    setTimeoutSeconds(role.timeout_seconds);
    setMaxOutputTokens(role.max_output_tokens);
    setLastTestResult((current) =>
      current?.role.connection_id === role.connection_id &&
      current.role.model_id === role.model_id &&
      current.role.last_conformance_at === role.last_conformance_at
        ? current
        : null,
    );
  }, [role]);

  const connection = roleConnection(connections, connectionId);
  const configuredConnection = roleConnection(
    connections,
    role.connection_id ?? "",
  );
  const connectionAdapter = adapters.find(
    (adapter) => adapter.id === connection?.adapter_id,
  );
  const configuredConnectionAdapter = adapters.find(
    (adapter) => adapter.id === configuredConnection?.adapter_id,
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
    maxOutputTokens === role.max_output_tokens;
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
        timeout_seconds: timeoutSeconds,
        max_output_tokens: maxOutputTokens,
      });
    } catch {
      // The parent reports the failure and preserves this draft for correction.
    }
  }

  async function testModel() {
    const result = await onTest(role.role_id);
    if (result !== null) setLastTestResult(result);
  }

  const stateLabel = roleStateLabel(
    role,
    configured,
    connection?.credential_saved === true,
    verifiedCapabilitiesSatisfied,
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
            disabled={busy || testing}
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
          <span className={`assistant-role-state${role.effective_enabled ? " is-ready" : ""}`}>
            {stateLabel}
          </span>
          <h3>{role.label}</h3>
        </div>
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
              setLastTestResult(null);
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
          <span className="field-hint">
            This choice applies only to {role.label.toLocaleLowerCase()}. Other
            tasks may reuse this key or choose a different connection.
          </span>
        </label>

        <div className="field">
          <label className="field-label" htmlFor={`assistant-model-${role.role_id}`}>
            Model
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

        <label className="checkbox-row assistant-role-enabled">
          <input
            type="checkbox"
            checked={enabled}
            disabled={!enabled && !canEnable}
            onChange={(event) => setEnabled(event.target.checked)}
          />
          <span>Allow this model for this task</span>
        </label>
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

        {configured && role.conformance_status !== "never" ? (
          <TestResultReport
            label={`${role.label} model test result`}
            report={modelTestReport(
              role,
              configuredConnection,
              configuredConnectionAdapter,
              lastTestResult,
            )}
            openByDefault={role.conformance_status === "failed"}
          />
        ) : null}

        <details className="assistant-role-limits">
          <summary>Request limits</summary>
          <div className="field-row">
            <label className="field">
              <span className="field-label">Timeout (seconds)</span>
              <input
                type="number"
                min={5}
                max={300}
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
                value={maxOutputTokens}
                onChange={(event) => {
                  setMaxOutputTokens(Number(event.target.value));
                  setEnabled(false);
                }}
              />
            </label>
          </div>
        </details>

        <div className="assistant-role-actions">
          <button
            className="btn-primary"
            type="submit"
            disabled={busy || testing || !connectionId || !selectedModelAvailable}
          >
            {busy ? "Saving…" : "Save task"}
          </button>
          {configured ? (
            <button
              className="btn-secondary"
              type="button"
              disabled={busy || testing || !canTest}
              onClick={() => void testModel()}
            >
              {testing ? "Testing…" : "Test model"}
            </button>
          ) : null}
          {configured ? (
            <button
              className="btn-ghost"
              type="button"
              disabled={busy || testing}
              onClick={() => void onRemove(role.role_id)}
            >
              Clear
            </button>
          ) : null}
        </div>
        {configured && !configurationMatches ? (
          <p className="field-hint">Save these changes before testing the model.</p>
        ) : null}
        {configured ? (
          <p className="field-hint">
            The test sends only a one-time synthetic challenge—no song or library data.
          </p>
        ) : null}
      </form>
    </article>
  );
}
