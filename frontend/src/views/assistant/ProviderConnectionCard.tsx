import { useEffect, useState } from "react";

import type {
  ProviderAdapter,
  ProviderCapability,
  ProviderConnection,
  ProviderConnectionUpdate,
} from "@/core/assistantProvidersApi";

import {
  providerAddressAfterAdapterChange,
  verificationFailureMessage,
  verificationStatusLabel,
} from "./providerUi";

interface Props {
  connection: ProviderConnection;
  assignedRoleLabels: string[];
  adapters: ProviderAdapter[];
  capabilities: ProviderCapability[];
  credentialStorageReady: boolean;
  busy: boolean;
  onUpdate: (
    connectionId: string,
    payload: ProviderConnectionUpdate,
  ) => Promise<void>;
  onVerify: (connectionId: string) => Promise<void>;
  onDeleteCredential: (connection: ProviderConnection) => Promise<void>;
  onDelete: (connection: ProviderConnection) => Promise<void>;
}

export function ProviderConnectionCard({
  connection,
  assignedRoleLabels,
  adapters,
  capabilities,
  credentialStorageReady,
  busy,
  onUpdate,
  onVerify,
  onDeleteCredential,
  onDelete,
}: Props) {
  const [name, setName] = useState(connection.name);
  const [adapterId, setAdapterId] = useState(connection.adapter_id);
  const [baseUrl, setBaseUrl] = useState(connection.base_url);
  const [apiKey, setApiKey] = useState("");
  const [allowPrivate, setAllowPrivate] = useState(
    connection.allow_private_network,
  );

  useEffect(() => {
    setName(connection.name);
    setAdapterId(connection.adapter_id);
    setBaseUrl(connection.base_url);
    setAllowPrivate(connection.allow_private_network);
  }, [connection]);

  async function save(event: React.FormEvent) {
    event.preventDefault();
    const payload: ProviderConnectionUpdate = {
      name: name.trim(),
      adapter_id: adapterId,
      base_url: baseUrl.trim(),
      allow_private_network: allowPrivate,
    };
    if (!connection.credential_saved && apiKey.trim()) {
      payload.api_key = apiKey.trim();
    }
    try {
      await onUpdate(connection.id, payload);
      setApiKey("");
    } catch {
      // The parent reports the failure and keeps this form open for correction.
    }
  }

  const models = connection.verified_models;
  const verifiedCapabilityLabels = connection.verified_capability_ids.map(
    (capabilityId) =>
      capabilities.find((capability) => capability.id === capabilityId)?.label ??
      capabilityId,
  );
  return (
    <details className="surface-card assistant-provider-card">
      <summary className="assistant-provider-card-summary">
        <span
          className={`assistant-provider-status is-${connection.verification_status}`}
        >
          {verificationStatusLabel(connection.verification_status)}
        </span>
        <span className="assistant-provider-card-identity">
          <h3>{connection.name}</h3>
          <small>{connection.base_url}</small>
        </span>
        <span className="assistant-provider-card-usage">
          <strong>{models.length} model{models.length === 1 ? "" : "s"}</strong>
          <small>{assignedRoleLabels.join(" · ") || "No assigned tasks"}</small>
        </span>
      </summary>
      <div className="assistant-provider-card-body">
      {connection.verification_status === "failed" ? (
        <p className="assistant-provider-problem" role="status">
          {verificationFailureMessage(connection.verification_error_code)}
        </p>
      ) : null}
      {connection.verification_status === "verified" &&
      verifiedCapabilityLabels.length === 0 ? (
        <p className="assistant-provider-problem" role="status">
          This connection cannot be assigned to a model task until verification
          confirms a compatible capability.
        </p>
      ) : null}

      <div className="assistant-provider-actions">
        <button
          className="btn-secondary"
          type="button"
          disabled={
            busy || !credentialStorageReady || !connection.credential_saved
          }
          onClick={() => void onVerify(connection.id)}
        >
          {busy
            ? "Working…"
            : connection.verification_status === "verified"
              ? "Verify again"
              : "Verify connection"}
        </button>
        <button
          className="btn-danger"
          type="button"
          disabled={busy}
          onClick={() => void onDelete(connection)}
        >
          Delete connection
        </button>
      </div>

      <div className="assistant-provider-details">
        <dl>
          <div>
            <dt>Saved key</dt>
            <dd>
              {connection.credential_saved
                ? connection.key_hint ?? "Saved"
                : "Missing"}
            </dd>
          </div>
          <div>
            <dt>Verified capabilities</dt>
            <dd>{verifiedCapabilityLabels.join(" · ") || "None confirmed"}</dd>
          </div>
          <div>
            <dt>Available models</dt>
            <dd>{models.join(" · ") || "Verify to load models"}</dd>
          </div>
        </dl>
        {connection.credential_saved ? (
          <button
            className="btn-ghost"
            type="button"
            disabled={busy}
            onClick={() => void onDeleteCredential(connection)}
          >
            Delete API key
          </button>
        ) : null}

        <details className="assistant-provider-settings">
          <summary>Connection settings</summary>
          <form
            className="assistant-provider-edit"
            onSubmit={(event) => void save(event)}
          >
            <label className="field">
              <span className="field-label">Connection name</span>
              <input
                value={name}
                maxLength={128}
                required
                onChange={(event) => setName(event.target.value)}
              />
            </label>
            <label className="field">
              <span className="field-label">Connection type</span>
              <select
                value={adapterId}
                onChange={(event) => {
                  const nextAdapterId = event.target.value;
                  setBaseUrl((current) =>
                    providerAddressAfterAdapterChange(
                      current,
                      adapterId,
                      nextAdapterId,
                    ),
                  );
                  setAdapterId(nextAdapterId);
                }}
              >
                {adapters.map((adapter) => (
                  <option key={adapter.id} value={adapter.id}>
                    {adapter.label}
                  </option>
                ))}
              </select>
              <small className="field-hint">
                {adapters.find((adapter) => adapter.id === adapterId)?.description}
              </small>
            </label>
            <label className="field">
              <span className="field-label">Provider address</span>
              <input
                type="url"
                value={baseUrl}
                maxLength={2048}
                required
                onChange={(event) => setBaseUrl(event.target.value)}
              />
            </label>
            {connection.credential_saved ? (
              <p className="field-hint">
                The saved API key cannot be replaced in place. Delete it first if
                you intentionally need to enter another key.
              </p>
            ) : (
              <label className="field">
                <span className="field-label">API key</span>
                <input
                  type="password"
                  value={apiKey}
                  maxLength={4096}
                  autoComplete="new-password"
                  disabled={!credentialStorageReady}
                  placeholder="Enter a key to enable verification"
                  onChange={(event) => setApiKey(event.target.value)}
                />
              </label>
            )}
            <label className="checkbox-row assistant-private-network">
              <input
                type="checkbox"
                checked={allowPrivate}
                onChange={(event) => setAllowPrivate(event.target.checked)}
              />
              <span>Allow a provider on my private network</span>
            </label>
            <p className="field-hint">
              Changing provider settings clears verification and assigned task
              checks. Active model work must finish or be cancelled first.
            </p>
            <button
              className="btn-primary"
              type="submit"
              disabled={busy || !name.trim() || !baseUrl.trim()}
            >
              Save changes
            </button>
          </form>
        </details>
      </div>
      </div>
    </details>
  );
}
