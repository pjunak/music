import { useCallback, useEffect, useState } from "react";

import { confirmDialog } from "@/components/confirmDialog";
import { inputDialog } from "@/components/inputDialog";
import { type BackgroundJob, jobsApi } from "@/core/api";
import type {
  ModelQualityEvaluation,
  ModelRole,
  ModelRoleUpdate,
  ProviderConnection,
  ProviderConnectionCreate,
  ProviderConnectionUpdate,
  ProviderFrameworkStatus,
} from "@/core/assistantProvidersApi";
import { assistantProvidersApi } from "@/core/assistantProvidersApi";
import { toast } from "@/core/toast";

import { CredentialStorageCard } from "./CredentialStorageCard";
import { ModelRoleCard } from "./ModelRoleCard";
import { ModelQualityEvaluationCard } from "./ModelQualityEvaluationCard";
import {
  isModelEvaluationJobActive,
  MODEL_QUALITY_TARGETS,
} from "./modelEvaluationJobs";
import { ProviderConnectionCard } from "./ProviderConnectionCard";
import {
  modelTestFailureMessage,
  verificationFailureMessage,
} from "./providerUi";

function errorMessage(error: unknown): string {
  return error instanceof Error ? error.message : "The request could not be completed.";
}

export function AssistantAiSetupView() {
  const [status, setStatus] = useState<ProviderFrameworkStatus | null>(null);
  const [connections, setConnections] = useState<ProviderConnection[]>([]);
  const [roles, setRoles] = useState<ModelRole[]>([]);
  const [loading, setLoading] = useState(true);
  const [loadError, setLoadError] = useState<string | null>(null);
  const [refreshKey, setRefreshKey] = useState(0);
  const [busyItem, setBusyItem] = useState<string | null>(null);
  const [storageInitializing, setStorageInitializing] = useState(false);
  const [storageResetting, setStorageResetting] = useState(false);
  const [qualityEvaluations, setQualityEvaluations] = useState<
    ModelQualityEvaluation[]
  >([]);
  const [qualityHistory, setQualityHistory] = useState<BackgroundJob[]>([]);
  const [qualityLoading, setQualityLoading] = useState(true);
  const [qualityLoadError, setQualityLoadError] = useState<string | null>(null);
  const [qualityRefreshKey, setQualityRefreshKey] = useState(0);

  const [name, setName] = useState("");
  const [adapterId, setAdapterId] = useState("");
  const [baseUrl, setBaseUrl] = useState("");
  const [apiKey, setApiKey] = useState("");
  const [allowPrivate, setAllowPrivate] = useState(false);

  const refresh = useCallback(() => setRefreshKey((value) => value + 1), []);
  const refreshQuality = useCallback(
    () => setQualityRefreshKey((value) => value + 1),
    [],
  );

  useEffect(() => {
    let disposed = false;
    setLoading(true);
    Promise.all([
      assistantProvidersApi.getStatus(),
      assistantProvidersApi.listConnections(),
      assistantProvidersApi.listRoles(),
    ])
      .then(([nextStatus, nextConnections, nextRoles]) => {
        if (disposed) return;
        setStatus(nextStatus);
        setConnections(nextConnections);
        setRoles(nextRoles);
        setAdapterId((current) => current || nextStatus.adapters[0]?.id || "");
        setLoadError(null);
      })
      .catch((error: unknown) => {
        if (!disposed) setLoadError(errorMessage(error));
      })
      .finally(() => {
        if (!disposed) setLoading(false);
      });
    return () => {
      disposed = true;
    };
  }, [refreshKey]);

  useEffect(() => {
    let disposed = false;
    let timer: number | undefined;

    async function poll(initial: boolean) {
      try {
        const targetResults = await Promise.all(
          MODEL_QUALITY_TARGETS.map(async (target) => {
            const [evaluations, history] = await Promise.all([
              assistantProvidersApi.listRoleEvaluations(target.roleId),
              jobsApi.list({ kind: target.jobKind, limit: 10 }),
            ]);
            return {
              evaluations: evaluations.filter(
                (evaluation) => evaluation.role_id === target.roleId,
              ),
              history: history.filter((job) => job.kind === target.jobKind),
            };
          }),
        );
        if (disposed) return;
        const nextEvaluations = targetResults.flatMap(
          (result) => result.evaluations,
        );
        const nextHistory = targetResults
          .flatMap((result) => result.history)
          .sort((left, right) => right.created_at.localeCompare(left.created_at));
        setQualityEvaluations(nextEvaluations);
        setQualityHistory(nextHistory);
        setQualityLoadError(null);
        const hasActiveJob = nextHistory.some(isModelEvaluationJobActive);
        timer = window.setTimeout(
          () => void poll(false),
          hasActiveJob ? 1500 : 5000,
        );
      } catch (error) {
        if (disposed) return;
        setQualityLoadError(errorMessage(error));
        timer = window.setTimeout(() => void poll(false), 5000);
      } finally {
        if (!disposed && initial) setQualityLoading(false);
      }
    }

    void poll(true);
    return () => {
      disposed = true;
      if (timer !== undefined) window.clearTimeout(timer);
    };
  }, [qualityRefreshKey]);

  async function initializeCredentialStorage() {
    setStorageInitializing(true);
    try {
      const nextStatus = await assistantProvidersApi.initializeCredentialStorage();
      setStatus(nextStatus);
      toast.success(
        "Encrypted storage initialized",
        "You can now save provider API keys.",
      );
    } catch (error) {
      toast.error("Encrypted storage could not be initialized", errorMessage(error));
      refresh();
    } finally {
      setStorageInitializing(false);
    }
  }

  async function resetCredentialStorage() {
    const confirmed = await confirmDialog({
      title: "Reset all AI secure storage?",
      body:
        "Every saved provider API key will be permanently deleted, all model verification and quality gates will reset, and the file-backed master key will be removed. Connection and task drafts will remain.",
      confirmLabel: "Continue to password",
      tone: "danger",
    });
    if (!confirmed) return;
    const currentPassword = await inputDialog({
      title: "Confirm AI storage reset",
      body:
        "Enter the password for your currently signed-in Music account. This password is checked by the server and is not stored.",
      label: "Current password",
      type: "password",
      confirmLabel: "Delete all AI credentials",
      trim: false,
    });
    if (currentPassword === null) return;

    setStorageResetting(true);
    try {
      const result = await assistantProvidersApi.resetCredentialStorage(
        currentPassword,
      );
      setStatus(result.status);
      refresh();
      refreshQuality();
      if (result.master_key_removed) {
        toast.success(
          "AI secure storage reset",
          `${result.deleted_credentials} saved provider API key${result.deleted_credentials === 1 ? " was" : "s were"} deleted. Connection and task drafts were kept.`,
        );
      } else {
        toast.error(
          "Provider API keys deleted, but the master key remains",
          "No credential still depends on that key. Retry the reset; if it still fails, SSH is needed to repair or remove the fixed key file.",
        );
      }
    } catch (error) {
      toast.error("AI secure storage could not be reset", errorMessage(error));
      refresh();
    } finally {
      setStorageResetting(false);
    }
  }

  async function createConnection(event: React.FormEvent) {
    event.preventDefault();
    if (!status?.credential_storage_ready) return;
    const payload: ProviderConnectionCreate = {
      name: name.trim(),
      adapter_id: adapterId,
      base_url: baseUrl.trim(),
      api_key: apiKey.trim(),
      allow_private_network: allowPrivate,
    };
    setBusyItem("create");
    try {
      const created = await assistantProvidersApi.createConnection(payload);
      setConnections((current) => [...current, created]);
      setName("");
      setBaseUrl("");
      setApiKey("");
      setAllowPrivate(false);
      toast.success("Connection saved", "Verify it before assigning any model tasks.");
    } catch (error) {
      toast.error("Connection could not be saved", errorMessage(error));
    } finally {
      setBusyItem(null);
    }
  }

  async function updateConnection(
    connectionId: string,
    payload: ProviderConnectionUpdate,
  ) {
    setBusyItem(`connection:${connectionId}`);
    try {
      const updated = await assistantProvidersApi.updateConnection(
        connectionId,
        payload,
      );
      setConnections((current) =>
        current.map((item) => (item.id === updated.id ? updated : item)),
      );
      toast.success("Connection updated");
      refresh();
      refreshQuality();
    } catch (error) {
      toast.error("Connection could not be updated", errorMessage(error));
      throw error;
    } finally {
      setBusyItem(null);
    }
  }

  async function verifyConnection(connectionId: string) {
    setBusyItem(`connection:${connectionId}`);
    try {
      const result = await assistantProvidersApi.verifyConnection(connectionId);
      setConnections((current) =>
        current.map((item) =>
          item.id === result.connection.id ? result.connection : item,
        ),
      );
      if (result.verified) {
        toast.success(
          "Connection verified",
          `${result.models.length} model${result.models.length === 1 ? "" : "s"} available.`,
        );
      } else {
        toast.error("Verification failed", verificationFailureMessage(result.error_code));
      }
      refresh();
      refreshQuality();
    } catch (error) {
      toast.error("Verification could not run", errorMessage(error));
    } finally {
      setBusyItem(null);
    }
  }

  async function deleteConnection(connection: ProviderConnection) {
    const confirmed = await confirmDialog({
      title: "Delete model connection?",
      body: `${connection.name} and its stored API key will be removed.`,
      confirmLabel: "Delete connection",
      tone: "danger",
    });
    if (!confirmed) return;
    setBusyItem(`connection:${connection.id}`);
    try {
      await assistantProvidersApi.deleteConnection(connection.id);
      setConnections((current) =>
        current.filter((item) => item.id !== connection.id),
      );
      toast.success("Connection deleted");
    } catch (error) {
      toast.error("Connection could not be deleted", errorMessage(error));
    } finally {
      setBusyItem(null);
    }
  }

  async function deleteConnectionCredential(connection: ProviderConnection) {
    const confirmed = await confirmDialog({
      title: "Delete saved API key?",
      body: `${connection.name} will remain, but its model tasks cannot run until you save and verify a new key.`,
      confirmLabel: "Delete API key",
      tone: "danger",
    });
    if (!confirmed) return;
    setBusyItem(`connection:${connection.id}`);
    try {
      const updated = await assistantProvidersApi.deleteConnectionCredential(
        connection.id,
      );
      setConnections((current) =>
        current.map((item) => (item.id === updated.id ? updated : item)),
      );
      toast.success(
        "API key deleted",
        "The connection was kept. Save and verify a new key before using it again.",
      );
      refresh();
      refreshQuality();
    } catch (error) {
      toast.error("API key could not be deleted", errorMessage(error));
    } finally {
      setBusyItem(null);
    }
  }

  async function saveRole(roleId: string, payload: ModelRoleUpdate) {
    setBusyItem(`role:${roleId}`);
    try {
      const updated = await assistantProvidersApi.updateRole(roleId, payload);
      setRoles((current) =>
        current.map((role) => (role.role_id === roleId ? updated : role)),
      );
      toast.success("Model task saved", updated.label);
      refreshQuality();
    } catch (error) {
      toast.error("Model task could not be saved", errorMessage(error));
      throw error;
    } finally {
      setBusyItem(null);
    }
  }

  async function removeRole(roleId: string) {
    setBusyItem(`role:${roleId}`);
    try {
      await assistantProvidersApi.deleteRole(roleId);
      toast.success("Model task cleared");
      refresh();
      refreshQuality();
    } catch (error) {
      toast.error("Model task could not be cleared", errorMessage(error));
    } finally {
      setBusyItem(null);
    }
  }

  async function testRole(roleId: string) {
    setBusyItem(`role-test:${roleId}`);
    try {
      const result = await assistantProvidersApi.testRole(roleId);
      setRoles((current) =>
        current.map((role) =>
          role.role_id === roleId ? result.role : role,
        ),
      );
      if (result.passed) {
        toast.success(
          "Model test passed",
          "You can now enable this model for its assigned task.",
        );
      } else {
        toast.error("Model test failed", modelTestFailureMessage(result.error_code));
      }
    } catch (error) {
      toast.error("Model test could not run", errorMessage(error));
    } finally {
      setBusyItem(null);
    }
  }

  async function startQualityEvaluation(evaluation: ModelQualityEvaluation) {
    const isMusicTagging = evaluation.role_id === "music_tagger";
    const isTagCleanup = evaluation.role_id === "tag_cleanup";
    const confirmed = await confirmDialog({
      title: isMusicTagging
        ? "Run music tagging model quality check?"
        : isTagCleanup
          ? "Run song-tag cleanup model quality check?"
          : "Run playlist model quality check?",
      body:
        `The provider will receive fixed synthetic ${
          isMusicTagging
            ? "music metadata cases"
            : isTagCleanup
              ? "tag-catalog cleanup cases"
              : "playlist scenarios"
        }. ` +
        "No songs or live library data are sent, but repeated model calls may incur cost.",
      confirmLabel: "Run quality check",
      tone: "primary",
    });
    if (!confirmed) return;
    setBusyItem(`evaluation:${evaluation.evaluation_id}`);
    try {
      const job = await assistantProvidersApi.startRoleEvaluation(
        evaluation.role_id,
        evaluation.evaluation_id,
      );
      setQualityHistory((current) => [
        job,
        ...current.filter((item) => item.id !== job.id),
      ]);
      toast.success(
        "Model quality check queued",
        "You can leave this page; progress is stored on the server.",
      );
      refreshQuality();
    } catch (error) {
      toast.error("Quality check could not start", errorMessage(error));
    } finally {
      setBusyItem(null);
    }
  }

  async function cancelQualityEvaluation(jobId: string) {
    setBusyItem("evaluation-cancel");
    try {
      const job = await jobsApi.cancel(jobId);
      setQualityHistory((current) => [
        job,
        ...current.filter((item) => item.id !== job.id),
      ]);
      refreshQuality();
    } catch (error) {
      toast.error("Cancellation failed", errorMessage(error));
    } finally {
      setBusyItem(null);
    }
  }

  if (loading && status === null) {
    return <div className="route-spinner">Loading optional AI setup…</div>;
  }

  if (loadError !== null && status === null) {
    return (
      <div className="assistant-provider-view">
        <div className="surface-card assistant-provider-load-error" role="alert">
          <h2>AI setup is unavailable</h2>
          <p>{loadError}</p>
          <button className="btn-primary" type="button" onClick={refresh}>
            Try again
          </button>
        </div>
      </div>
    );
  }

  if (status === null) return null;

  return (
    <div className="assistant-provider-view">
      <header className="assistant-page-header">
        <div>
          <p className="assistant-eyebrow">Optional model routing</p>
          <h1>AI connections</h1>
          <p>
            Each connection stores one provider key. Reuse a connection when tasks
            should share that key, or create separate connections—even for the same
            provider—when a task needs its own key or account. Every Assistant task
            then chooses and tests its own connection and model.
          </p>
        </div>
        <span className="assistant-algorithm">local tools stay active</span>
      </header>

      <ol className="assistant-provider-path" aria-label="Connection setup steps">
        <li>
          <span>1</span>
          <strong>Connect</strong>
          <p>Save an address and API key.</p>
        </li>
        <li>
          <span>2</span>
          <strong>Verify</strong>
          <p>Check access and load model names.</p>
        </li>
        <li>
          <span>3</span>
          <strong>Assign</strong>
          <p>Choose one model for one task.</p>
        </li>
        <li>
          <span>4</span>
          <strong>Test &amp; enable</strong>
          <p>Prove structured output before use.</p>
        </li>
        <li>
          <span>5</span>
          <strong>Evaluate</strong>
          <p>Run task-specific quality checks.</p>
        </li>
      </ol>

      <section className="assistant-provider-section">
        <div className="assistant-section-heading">
          <div>
            <h2>Provider connections</h2>
            <p>
              Keys are encrypted by the server and are never shown again. Connection
              names help distinguish separate credentials, billing scopes, or model
              services.
            </p>
          </div>
          <span>{connections.length} saved</span>
        </div>

        <CredentialStorageCard
          status={status}
          busy={storageInitializing}
          resetting={storageResetting}
          onInitialize={initializeCredentialStorage}
          onReset={resetCredentialStorage}
        />
        <div
          className={`assistant-provider-connections-layout${
            status.credential_storage_ready ? "" : " is-storage-locked"
          }`}
        >
          {status.credential_storage_ready ? (
            <form
              className="surface-card assistant-provider-create"
              onSubmit={(event) => void createConnection(event)}
            >
              <div>
                <p className="assistant-eyebrow">New connection</p>
                <h3>Connect a model provider</h3>
              </div>
              <label className="field">
                <span className="field-label">Connection name</span>
                <input
                  value={name}
                  maxLength={128}
                  placeholder="For example: My hosted models"
                  required
                  onChange={(event) => setName(event.target.value)}
                />
              </label>
              <label className="field">
                <span className="field-label">Connection type</span>
                <select
                  value={adapterId}
                  required
                  onChange={(event) => setAdapterId(event.target.value)}
                >
                  {status.adapters.map((adapter) => (
                    <option key={adapter.id} value={adapter.id}>
                      {adapter.label}
                    </option>
                  ))}
                </select>
                <span className="field-hint">
                  {status.adapters.find((adapter) => adapter.id === adapterId)
                    ?.description ?? "Choose how this provider exposes its models."}
                </span>
                <span className="field-hint">
                  Supports: {status.adapters
                    .find((adapter) => adapter.id === adapterId)
                    ?.capability_ids.map(
                      (capabilityId) =>
                        status.capabilities.find(
                          (capability) => capability.id === capabilityId,
                        )?.label ?? capabilityId,
                    )
                    .join(" · ") || "No model-task capabilities"}
                </span>
              </label>
              <label className="field">
                <span className="field-label">Provider address</span>
                <input
                  type="url"
                  value={baseUrl}
                  maxLength={2048}
                  placeholder="https://provider.example/v1"
                  required
                  onChange={(event) => setBaseUrl(event.target.value)}
                />
              </label>
              <label className="field">
                <span className="field-label">API key</span>
                <input
                  type="password"
                  value={apiKey}
                  maxLength={4096}
                  autoComplete="new-password"
                  required
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
              {allowPrivate ? (
                <p className="assistant-provider-private-note">
                  Use this only for a model service you control on your own network.
                </p>
              ) : null}
              <button
                className="btn-primary"
                type="submit"
                disabled={
                  busyItem !== null ||
                  !name.trim() ||
                  !adapterId ||
                  !baseUrl.trim() ||
                  !apiKey.trim()
                }
              >
                {busyItem === "create" ? "Saving…" : "Save connection"}
              </button>
            </form>
          ) : null}

          <div className="assistant-provider-card-list">
            {connections.length === 0 ? (
              <div className="surface-card assistant-provider-empty">
                <h3>No provider connections yet</h3>
                <p>
                  Add one here when you are ready. Nothing leaves the server until you
                  explicitly verify a connection.
                </p>
              </div>
            ) : (
              connections.map((connection) => (
                <ProviderConnectionCard
                  key={connection.id}
                  connection={connection}
                  assignedRoleLabels={roles
                    .filter((role) => role.connection_id === connection.id)
                    .map((role) => role.label)}
                  adapters={status.adapters}
                  capabilities={status.capabilities}
                  credentialStorageReady={status.credential_storage_ready}
                  busy={busyItem === `connection:${connection.id}`}
                  onUpdate={updateConnection}
                  onVerify={verifyConnection}
                  onDeleteCredential={deleteConnectionCredential}
                  onDelete={deleteConnection}
                />
              ))
            )}
          </div>
        </div>
      </section>

      <section className="assistant-provider-section assistant-role-section">
        <div className="assistant-section-heading">
          <div>
            <h2>Model tasks</h2>
            <p>
              Each task independently chooses a connection and model. Several tasks
              may reuse one connection, while specialized tasks may use separate
              keys. The server permits only compatible, verified connections. Every
              available model task must also pass its own synthetic test before you
              can enable it; planned tasks remain locked until their complete safety
              and review contract exists.
            </p>
          </div>
          <span>{roles.filter((role) => role.effective_enabled).length} enabled</span>
        </div>
        {connections.length === 0 ? (
          <div className="surface-card assistant-provider-empty">
            <h3>Connections come first</h3>
            <p>Save and verify a provider above before assigning model tasks.</p>
          </div>
        ) : (
          <div className="assistant-role-grid">
            {roles.map((role) => (
              <ModelRoleCard
                key={role.role_id}
                role={role}
                connections={connections}
                adapters={status.adapters}
                capabilities={status.capabilities}
                credentialStorageReady={status.credential_storage_ready}
                busy={busyItem === `role:${role.role_id}`}
                testing={busyItem === `role-test:${role.role_id}`}
                onSave={saveRole}
                onTest={testRole}
                onRemove={removeRole}
              />
            ))}
          </div>
        )}
      </section>

      <section className="assistant-provider-section assistant-quality-section">
        <div className="assistant-section-heading">
          <div>
            <h2>Model quality checks</h2>
            <p>
              Basic model tests prove the response format. These longer checks
              measure each model against fixed task-specific scenarios before it can
              use live library metadata for that task.
            </p>
          </div>
          <span>
            {qualityEvaluations.filter((item) => item.status === "passed").length}{" "}
            passed
          </span>
        </div>
        {qualityLoadError !== null ? (
          <div className="assistant-analysis-error" role="alert">
            <span>{qualityLoadError}</span>
            <button type="button" onClick={refreshQuality}>
              Retry
            </button>
          </div>
        ) : null}
        {qualityEvaluations.length === 0 && qualityLoading ? (
          <div className="surface-card assistant-provider-empty">
            <p>Loading model quality checks…</p>
          </div>
        ) : qualityEvaluations.length === 0 && qualityLoadError === null ? (
          <div className="surface-card assistant-provider-empty">
            <h3>No quality checks are registered</h3>
            <p>Task-specific checks will appear here when the server provides them.</p>
          </div>
        ) : (
          qualityEvaluations.map((evaluation) => (
            <ModelQualityEvaluationCard
              key={`${evaluation.role_id}:${evaluation.evaluation_id}`}
              evaluation={evaluation}
              role={roles.find((role) => role.role_id === evaluation.role_id)}
              history={qualityHistory.filter(
                (job) =>
                  job.parameters.role_id === evaluation.role_id &&
                  job.parameters.evaluation_id === evaluation.evaluation_id,
              )}
              loading={qualityLoading}
              actionBusy={busyItem?.startsWith("evaluation") === true}
              onStart={() => void startQualityEvaluation(evaluation)}
              onCancel={(jobId) => void cancelQualityEvaluation(jobId)}
            />
          ))
        )}
      </section>

      <aside className="assistant-provider-boundary">
        <strong>Setup does not share your library</strong>
        <p>
          Verification, model tests, and quality checks use only provider metadata
          or fixed synthetic inputs. No songs, audio, filesystem paths, or live
          library tags are sent here. Passed models become optional choices in
          Playlist Builder or Library Analysis. Each real request has its own data
          disclosure and confirmation before anything is sent.
        </p>
      </aside>
    </div>
  );
}
