import { useEffect, useState } from "react";

import type {
  ModelRole,
  ModelRoleUpdate,
  ProviderConnection,
} from "@/core/assistantProvidersApi";

import { modelTestFailureMessage, roleConnection } from "./providerUi";

interface Props {
  role: ModelRole;
  connections: ProviderConnection[];
  credentialStorageReady: boolean;
  busy: boolean;
  testing: boolean;
  onSave: (roleId: string, payload: ModelRoleUpdate) => Promise<void>;
  onTest: (roleId: string) => Promise<void>;
  onRemove: (roleId: string) => Promise<void>;
}

export function ModelRoleCard({
  role,
  connections,
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
    connection?.credential_saved === true &&
    connection?.verification_status === "verified";
  const canEnable =
    canTest && role.conformance_status === "passed";
  const listId = `assistant-models-${role.role_id}`;

  async function save(event: React.FormEvent) {
    event.preventDefault();
    if (!connectionId || !modelId.trim()) return;
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

  const stateLabel = role.effective_enabled
    ? "Enabled"
    : role.enabled
      ? connection?.credential_saved !== true
        ? "API key removed"
        : role.verification_status !== "verified"
        ? "Waiting for verification"
        : role.conformance_status !== "passed"
          ? "Needs model test"
          : "Stored key unavailable"
      : configured
        ? role.conformance_status === "passed"
          ? "Tested, switched off"
          : role.conformance_status === "failed"
            ? "Model test failed"
            : "Configured, not tested"
        : "Not configured";

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
            {connections.map((item) => (
              <option key={item.id} value={item.id}>
                {item.name} · {item.credential_saved && item.key_hint
                  ? item.key_hint
                  : "no key saved"}
                {item.credential_saved && item.verification_status === "verified"
                  ? " · verified"
                  : ""}
              </option>
            ))}
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
              : connection.verification_status !== "verified"
                ? "Verify the selected connection before enabling it."
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
