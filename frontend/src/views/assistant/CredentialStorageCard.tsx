import { useState } from "react";

import type { ProviderFrameworkStatus } from "@/core/assistantProvidersApi";
import { toast } from "@/core/toast";

interface CredentialStorageCardProps {
  status: ProviderFrameworkStatus;
  busy: boolean;
  onInitialize: () => Promise<void>;
}

const CONTAINER_NAME = "music";
const HOST_SECRETS_DIRECTORY = "/srv/music-secrets";
const CONTAINER_SECRETS_DIRECTORY = "/run/music-secrets";

function shellQuote(value: string): string {
  return `'${value.replaceAll("'", `'"'"'`)}'`;
}

function storageTitle(status: ProviderFrameworkStatus): string {
  if (status.credential_storage_ready) return "Encrypted key storage is ready";
  if (
    status.credential_storage_initialization_error ===
    "saved_credentials_require_existing_key"
  ) {
    return "Restore the existing storage key";
  }
  if (status.credential_storage_error === "invalid_master_key") {
    return "The server's credential key is invalid";
  }
  if (
    status.credential_storage_error?.startsWith("master_key_file_") ||
    status.credential_storage_error?.startsWith("master_key_directory_")
  ) {
    return "The secure key file is unavailable";
  }
  if (status.credential_storage_error === "master_key_file_permissions") {
    return (
      "Restrict the configured key file to its owner (mode 0600), make it " +
      "readable by the container user, and restart the server."
    );
  }
  if (
    status.credential_storage_error === "master_key_file_unsafe" ||
    status.credential_storage_error === "master_key_file_unreadable"
  ) {
    return (
      "The configured path must be a readable regular file, not a symlink. " +
      "Correct the mounted secret and restart the server."
    );
  }
  if (status.credential_storage_can_initialize) {
    return "Initialize encrypted key storage";
  }
  if (
    status.credential_storage_initialization_error ===
    "master_key_file_not_configured"
  ) {
    return (
      "Configure ASSISTANT_CREDENTIAL_KEY_FILE or a deployment-managed " +
      "ASSISTANT_CREDENTIAL_KEY, then restart the server. Local tools remain available."
    );
  }
  return "Secure key storage needs server setup";
}

function storageDescription(status: ProviderFrameworkStatus): string {
  if (status.credential_storage_ready) {
    if (status.credential_storage_source === "file") {
      return (
        "The server is using a private key file from its mounted secrets " +
        "directory. Provider API keys can now be saved."
      );
    }
    return (
      "The master key is supplied by the server environment. Provider API " +
      "keys can now be saved."
    );
  }
  if (
    status.credential_storage_initialization_error ===
    "saved_credentials_require_existing_key"
  ) {
    return (
      "Encrypted provider keys already exist, so Music will not generate a " +
      "different master key. Restore the matching key file before continuing."
    );
  }
  if (status.credential_storage_error === "invalid_master_key") {
    return (
      "The configured value must be URL-safe base64 containing exactly 32 " +
      "bytes. Correct the server secret, then return here."
    );
  }
  if (status.credential_storage_can_initialize) {
    return (
      "The private secrets directory is mounted and writable. Music can " +
      "generate the master key there without showing it in the browser."
    );
  }
  return (
    "Mount the dedicated private secrets directory and restart the server. " +
    "Local analysis and playlist building remain available meanwhile."
  );
}

async function copyCommand(command: string): Promise<void> {
  try {
    await navigator.clipboard.writeText(command);
    toast.success("Command copied to clipboard");
  } catch {
    toast.error("Copy failed", "Clipboard access was blocked.");
  }
}

export function CredentialStorageCard({
  status,
  busy,
  onInitialize,
}: CredentialStorageCardProps) {
  const [showGuide, setShowGuide] = useState(false);
  const keyFilePath = status.credential_storage_key_file_path;
  const removeCommand =
    keyFilePath && status.credential_storage_source === "file"
      ? `sudo docker exec -u 0 ${CONTAINER_NAME} rm -- ${shellQuote(keyFilePath)}`
      : null;
  const initializationError = status.credential_storage_initialization_error;
  const needsMountSetup =
    !status.credential_storage_ready &&
    (initializationError === "master_key_file_not_configured" ||
      initializationError?.startsWith("master_key_directory_"));
  const prepareDirectoryCommand =
    `sudo install -d -m 0700 -o 1000 -g 1000 ${HOST_SECRETS_DIRECTORY}`;
  const dockerMount =
    `-v ${HOST_SECRETS_DIRECTORY}:${CONTAINER_SECRETS_DIRECTORY}`;

  return (
    <section
      className={`surface-card assistant-provider-storage${
        status.credential_storage_ready ? " is-ready" : ""
      }`}
      aria-labelledby="assistant-credential-storage-title"
    >
      <div aria-hidden="true">Key</div>
      <div className="assistant-provider-storage-body">
        <div className="assistant-provider-storage-summary">
          <div>
            <h3 id="assistant-credential-storage-title">{storageTitle(status)}</h3>
            <p>
              {storageDescription(status)}{" "}
              {!status.credential_storage_ready &&
              status.credential_storage_can_initialize
                ? "Local tools do not depend on this key."
                : null}
            </p>
            {status.credential_storage_key_id ? (
              <p className="assistant-provider-storage-meta">
                Non-secret key ID: <code>{status.credential_storage_key_id}</code>
              </p>
            ) : null}
          </div>
          <div className="assistant-provider-storage-actions">
            {status.credential_storage_can_initialize ? (
              <button
                className="btn-primary"
                type="button"
                disabled={busy}
                onClick={() => void onInitialize()}
              >
                {busy ? "Initializing…" : "Initialize secure storage"}
              </button>
            ) : null}
            <button
              className="btn-ghost"
              type="button"
              aria-expanded={showGuide}
              onClick={() => setShowGuide((current) => !current)}
            >
              {showGuide ? "Hide maintenance guide" : "Show maintenance guide"}
            </button>
          </div>
        </div>

        {showGuide ? (
          <div className="assistant-provider-storage-guide">
            <h4>Server maintenance</h4>
            <p>
              Music deliberately cannot replace or remove the master key from the
              browser. This prevents an accidental click from making every saved
              provider credential unreadable.
            </p>

            {status.credential_storage_source === "environment" ? (
              <p>
                This deployment supplies <code>ASSISTANT_CREDENTIAL_KEY</code> when
                the server starts. Change or remove it in the container or service
                configuration, then recreate or restart the server. A running app
                cannot persistently change its own environment.
              </p>
            ) : null}

            {needsMountSetup ? (
              <div className="assistant-provider-command-block">
                <p>
                  On the Docker host, prepare a private directory owned by the image's
                  user, then mount <code>{HOST_SECRETS_DIRECTORY}</code> at{" "}
                  <code>{CONTAINER_SECRETS_DIRECTORY}</code> and restart the container.
                </p>
                <div>
                  <code>{prepareDirectoryCommand}</code>
                  <button
                    className="btn-ghost"
                    type="button"
                    onClick={() => void copyCommand(prepareDirectoryCommand)}
                  >
                    Copy command
                  </button>
                </div>
                <p>
                  Docker mount: <code>{dockerMount}</code>
                </p>
                {initializationError === "master_key_file_not_configured" ? (
                  <p>
                    Also set <code>ASSISTANT_CREDENTIAL_KEY_FILE</code> to{" "}
                    <code>{keyFilePath ?? `${CONTAINER_SECRETS_DIRECTORY}/assistant-credential.key`}</code>.
                  </p>
                ) : null}
              </div>
            ) : null}

            {removeCommand ? (
              <div className="assistant-provider-command-block">
                <h5>Remove and start over</h5>
                <p>
                  First delete every saved provider API key in this screen. Only then,
                  if you intentionally want to abandon the current master key, sign in
                  to the Docker host and run:
                </p>
                <div>
                  <code>{removeCommand}</code>
                  <button
                    className="btn-ghost"
                    type="button"
                    onClick={() => void copyCommand(removeCommand)}
                  >
                    Copy command
                  </button>
                </div>
                <p>
                  This assumes the documented container name{" "}
                  <code>{CONTAINER_NAME}</code>. Adjust it if your deployment uses
                  another name.
                </p>
              </div>
            ) : null}

            <p>
              To change the key while keeping saved provider credentials, use the
              offline <code>music-cli assistant-credentials rotate</code> workflow.
              Never edit or replace the key file directly: the database and master key
              must stay a matching pair.
            </p>
          </div>
        ) : null}
      </div>
    </section>
  );
}
