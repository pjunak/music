import { useEffect, useState } from "react";

import type {
  ProviderAdapter,
  ProviderCapability,
  ProviderConnection,
  ProviderConnectionUpdate,
} from "@/core/assistantProvidersApi";

import {
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
  const [editing, setEditing] = useState(false);
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
    if (apiKey.trim()) payload.api_key = apiKey.trim();
    try {
      await onUpdate(connection.id, payload);
      setApiKey("");
      setEditing(false);
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
    <article className="surface-card assistant-provider-card">
      <div className="assistant-provider-card-heading">
        <div>
          <span
            className={`assistant-provider-status is-${connection.verification_status}`}
          >
            {verificationStatusLabel(connection.verification_status)}
          </span>
          <h3>{connection.name}</h3>
        </div>
      </div>

      <p className="assistant-provider-url">{connection.base_url}</p>
      <div
        className={`assistant-provider-credential${
          connection.credential_saved ? " is-saved" : " is-missing"
        }`}
      >
        <div>
          <strong>
            {connection.credential_saved ? "API key saved" : "No API key saved"}
          </strong>
          <p>
            {connection.credential_saved
              ? connection.verification_status === "verified"
                ? verifiedCapabilityLabels.length > 0
                  ? `Encrypted on this server. Verified for: ${verifiedCapabilityLabels.join(
                      " · ",
                    )}.`
                  : "Encrypted on this server, but no supported task capability was confirmed."
                : "Encrypted on this server. Verification is still required before use."
              : "Add a key under Change, then verify this connection before use."}
          </p>
        </div>
        {connection.credential_saved && connection.key_hint ? (
          <code>{connection.key_hint}</code>
        ) : null}
      </div>
      <p className="field-hint">
        {assignedRoleLabels.length > 0
          ? `Used by: ${assignedRoleLabels.join(" · ")}`
          : "Not assigned to an AI task yet."}
      </p>
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

      <div className="assistant-provider-models">
        <span>Available models</span>
        <strong>{models.length}</strong>
        {models.length > 0 ? (
          <p title={models.join(", ")}>{models.slice(0, 3).join(" · ")}</p>
        ) : (
          <p>
            {connection.credential_saved
              ? "Verify this connection to load its model list."
              : "Save an API key before loading models."}
          </p>
        )}
      </div>

      <div className="assistant-provider-actions">
        <button
          className="btn-secondary"
          type="button"
          disabled={
            busy || !credentialStorageReady || !connection.credential_saved
          }
          onClick={() => void onVerify(connection.id)}
        >
          {busy ? "Working…" : "Verify connection"}
        </button>
        <button
          className="btn-ghost"
          type="button"
          disabled={busy}
          aria-expanded={editing}
          onClick={() => setEditing((value) => !value)}
        >
          {editing ? "Close changes" : "Change"}
        </button>
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
        <button
          className="btn-danger"
          type="button"
          disabled={busy}
          onClick={() => void onDelete(connection)}
        >
          Delete
        </button>
      </div>

      {editing ? (
        <form className="assistant-provider-edit" onSubmit={(event) => void save(event)}>
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
              onChange={(event) => setAdapterId(event.target.value)}
            >
              {adapters.map((adapter) => (
                <option key={adapter.id} value={adapter.id}>
                  {adapter.label}
                </option>
              ))}
            </select>
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
          <label className="field">
            <span className="field-label">
              {connection.credential_saved ? "Replace API key" : "API key"}
            </span>
            <input
              type="password"
              value={apiKey}
              maxLength={4096}
              autoComplete="new-password"
              disabled={!credentialStorageReady}
              placeholder={
                connection.credential_saved
                  ? "Leave empty to keep the current key"
                  : "Enter a key to enable verification"
              }
              onChange={(event) => setApiKey(event.target.value)}
            />
          </label>
          <label className="checkbox-row assistant-private-network">
            <input
              type="checkbox"
              checked={allowPrivate}
              onChange={(event) => setAllowPrivate(event.target.checked)}
            />
            <span>Allow a provider on my private network</span>
          </label>
          <p className="field-hint">
            Saving or replacing a key—or changing the address, connection type, or
            network access—requires verification again.
          </p>
          <button
            className="btn-primary"
            type="submit"
            disabled={busy || !name.trim() || !baseUrl.trim()}
          >
            Save changes
          </button>
        </form>
      ) : null}
    </article>
  );
}
