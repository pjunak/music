import { useEffect, useState } from "react";

import type { ProviderFrameworkStatus } from "@/core/assistantProvidersApi";
import { toast } from "@/core/toast";

interface CredentialStorageCardProps {
  status: ProviderFrameworkStatus;
  busy: boolean;
  resetting: boolean;
  onInitialize: () => Promise<void>;
  onReset: () => Promise<void>;
}

const DEFAULT_HOST_SECRETS_DIRECTORY = "/srv/music-secrets";
const CONTAINER_SECRETS_DIRECTORY = "/run/music-secrets";

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
  resetting,
  onInitialize,
  onReset,
}: CredentialStorageCardProps) {
  const [showGuide, setShowGuide] = useState(false);
  const [storageOpen, setStorageOpen] = useState(
    !status.credential_storage_ready,
  );

  useEffect(() => {
    setStorageOpen(!status.credential_storage_ready);
  }, [status.credential_storage_ready]);
  const keyFilePath = status.credential_storage_key_file_path;
  const hostSecretsDirectory =
    status.credential_storage_host_directory_hint ??
    DEFAULT_HOST_SECRETS_DIRECTORY;
  const initializationError = status.credential_storage_initialization_error;
  const resetAvailable =
    status.credential_storage_source === "file" ||
    (keyFilePath !== null &&
      initializationError === "saved_credentials_require_existing_key");
  const needsMountSetup =
    !status.credential_storage_ready &&
    (initializationError === "master_key_file_not_configured" ||
      initializationError?.startsWith("master_key_directory_"));
  const prepareDirectoryCommand =
    `sudo install -d -m 0700 -o 1000 -g 1000 ${hostSecretsDirectory}`;
  const dockerMount =
    `-v ${hostSecretsDirectory}:${CONTAINER_SECRETS_DIRECTORY}`;

  return (
    <details
      className={`surface-card assistant-provider-storage${
        status.credential_storage_ready ? " is-ready" : ""
      }`}
      open={storageOpen}
      onToggle={(event) => setStorageOpen(event.currentTarget.open)}
      aria-labelledby="assistant-credential-storage-title"
    >
      <summary>
        <span aria-hidden="true">Key</span>
        <span id="assistant-credential-storage-title">{storageTitle(status)}</span>
        <small>
          {status.credential_storage_ready ? "Ready" : "Action required"}
        </small>
      </summary>
      <div className="assistant-provider-storage-body">
        <div className="assistant-provider-storage-summary">
          <div>
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
                disabled={busy || resetting}
                onClick={() => void onInitialize()}
              >
                {busy ? "Initializing…" : "Initialize secure storage"}
              </button>
            ) : null}
            {resetAvailable ? (
              <button
                className="btn-danger"
                type="button"
                disabled={busy || resetting}
                onClick={() => void onReset()}
              >
                {resetting ? "Resetting…" : "Reset AI secure storage"}
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
              File-backed storage can be reset above after an explicit warning and
              current-password confirmation. Music erases every saved provider key
              before it removes the fixed master-key file, so the reset cannot leave
              encrypted credentials orphaned.
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
                  user, then mount <code>{hostSecretsDirectory}</code> at{" "}
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

            <p>
              To change the key while keeping saved provider credentials, use the
              offline <code>music-cli assistant-credentials rotate</code> workflow.
              Never edit or replace the key file directly: the database and master key
              must stay a matching pair.
            </p>
          </div>
        ) : null}
      </div>
    </details>
  );
}
