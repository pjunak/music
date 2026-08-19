import { useEffect, useState } from "react";

import type {
  ModelRole,
  ModelRoleUpdate,
  ProviderAdapter,
  ProviderCapability,
  ProviderConnection,
} from "@/core/assistantProvidersApi";

import { modelTestFailureMessage, roleConnection } from "./providerUi";

interface Props {
  role: ModelRole;
  connections: ProviderConnection[];
  adapters: ProviderAdapter[];
  capabilities: ProviderCapability[];
  credentialStorageReady: boolean;
  busy: boolean;
  testing: boolean;
  onSave: (roleId: string, payload: ModelRoleUpdate) => Promise<void>;
  onTest: (roleId: string) => Promise<void>;
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

  useEffect(() => {
    setConnectionId(role.connection_id ?? "");
    setModelId(role.model_id);
    setEnabled(role.enabled);
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
    verifiedCapabilitiesSatisfied;
  const canEnable =
    canTest && role.conformance_status === "passed";
  const listId = `assistant-models-${role.role_id}`;

  async function save(event: React.FormEvent) {
    event.preventDefault();
    if (!role.configuration_available || !connectionId || !modelId.trim()) return;
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

  const stateLabel = roleStateLabel(
    role,
    configured,
    connection?.credential_saved === true,
    verifiedCapabilitiesSatisfied,
  );

  if (!role.configuration_available) {
    return (
      <article className="surface-card assistant-role-card is-planned">
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
    <article className="surface-card assistant-role-card">
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
              setConnectionId(event.target.value);
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
          <span className="field-hint">
            This choice applies only to {role.label.toLocaleLowerCase()}. Other
            tasks may reuse this key or choose a different connection.
          </span>
        </label>

        <label className="field">
          <span className="field-label">Model</span>
          <input
            value={modelId}
            list={listId}
            maxLength={256}
            placeholder={
              connection?.verified_models.length
                ? "Choose or type a model ID"
                : "Enter the provider's model ID"
            }
            onChange={(event) => {
              setModelId(event.target.value);
              setEnabled(false);
            }}
          />
          <datalist id={listId}>
            {connection?.verified_models.map((item) => (
              <option key={item} value={item} />
            ))}
          </datalist>
        </label>

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
            disabled={busy || testing || !connectionId || !modelId.trim()}
          >
            {busy ? "Saving…" : "Save task"}
          </button>
          {configured ? (
            <button
              className="btn-secondary"
              type="button"
              disabled={busy || testing || !canTest}
              onClick={() => void onTest(role.role_id)}
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
