import { useCallback, useEffect, useState } from "react";

import { confirmDialog } from "@/components/confirmDialog";
import { inputDialog } from "@/components/inputDialog";
import { type BackgroundJob, jobsApi } from "@/core/api";
import type {
  ModelConformance,
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
import { AssistantInfoPopover } from "./AssistantInfoPopover";
import { ModelRoleCard } from "./ModelRoleCard";
import { ModelTestConsole } from "./ModelTestConsole";
import {
  isModelEvaluationJobActive,
  MODEL_QUALITY_TARGETS,
} from "./modelEvaluationJobs";
import { ProviderConnectionCard } from "./ProviderConnectionCard";
import {
  defaultProviderAddress,
  modelTestFailureMessage,
  providerAddressAfterAdapterChange,
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
  const [selectedTestRoleId, setSelectedTestRoleId] = useState<string | null>(
    null,
  );
  const [testConsoleOpen, setTestConsoleOpen] = useState(false);
  const [modelTestResults, setModelTestResults] = useState<
    Record<string, ModelConformance | undefined>
  >({});
  const [modelTestErrors, setModelTestErrors] = useState<
    Record<string, string | undefined>
  >({});

  const [name, setName] = useState("");
  const [adapterId, setAdapterId] = useState("");
  const [baseUrl, setBaseUrl] = useState("");
  const [apiKey, setApiKey] = useState("");
  const [allowPrivate, setAllowPrivate] = useState(false);
  const [setupHubOpen, setSetupHubOpen] = useState(true);

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
        setSetupHubOpen(nextConnections.length === 0);
        const defaultAdapterId = nextStatus.adapters[0]?.id || "";
        setAdapterId((current) => current || defaultAdapterId);
        setBaseUrl((current) => current || defaultProviderAddress(defaultAdapterId));
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
        if (
          initial &&
          (nextHistory.some(isModelEvaluationJobActive) ||
            nextEvaluations.some((evaluation) => evaluation.status === "failed"))
        ) {
          setTestConsoleOpen(true);
        }
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

  useEffect(() => {
    setSelectedTestRoleId((current) => {
      if (
        current !== null &&
        roles.some(
          (role) => role.role_id === current && role.configuration_available,
        )
      ) {
        return current;
      }
      const activeRoleId = qualityHistory.find(isModelEvaluationJobActive)
        ?.parameters.role_id;
      if (typeof activeRoleId === "string") return activeRoleId;
      const firstEvaluationRole = qualityEvaluations[0]?.role_id;
      if (firstEvaluationRole !== undefined) return firstEvaluationRole;
      return roles.find((role) => role.configuration_available)?.role_id ?? null;
    });
  }, [qualityEvaluations, qualityHistory, roles]);

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
      setSetupHubOpen(false);
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
    const connection = connections.find((item) => item.id === connectionId);
    const assignedRoles = roles.filter(
      (role) => role.connection_id === connectionId,
    );
    if (
      connection?.verification_status === "verified" &&
      assignedRoles.length > 0
    ) {
      const confirmed = await confirmDialog({
        title: "Verify connection again?",
        body:
          `Verifying ${connection.name} again will clear the model tests and quality results for ` +
          `${assignedRoles.map((role) => role.label).join(", ")}. ` +
          "Wait for or cancel any running model work first.",
        confirmLabel: "Verify and reset tests",
        tone: "primary",
      });
      if (!confirmed) return;
    }
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
      const remainingConnections = connections.filter(
        (item) => item.id !== connection.id,
      );
      setConnections(remainingConnections);
      if (remainingConnections.length === 0) setSetupHubOpen(true);
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
      setModelTestResults((current) => {
        const next = { ...current };
        delete next[roleId];
        return next;
      });
      setModelTestErrors((current) => {
        const next = { ...current };
        delete next[roleId];
        return next;
      });
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
    setSelectedTestRoleId(roleId);
    setTestConsoleOpen(true);
    setModelTestErrors((current) => {
      const next = { ...current };
      delete next[roleId];
      return next;
    });
    setBusyItem(`role-test:${roleId}`);
    try {
      const result = await assistantProvidersApi.testRole(roleId);
      setModelTestResults((current) => ({ ...current, [roleId]: result }));
      setRoles((current) =>
        current.map((role) =>
          role.role_id === roleId ? result.role : role,
        ),
      );
      if (result.passed) {
        try {
          const testedConnectionId = result.role.connection_id;
          if (testedConnectionId === null) {
            throw new Error("The tested task no longer has a connection.");
          }
          const allowed = await assistantProvidersApi.updateRole(roleId, {
            connection_id: testedConnectionId,
            model_id: result.role.model_id,
            enabled: true,
            thinking_mode: result.role.thinking_mode,
            timeout_seconds: result.role.timeout_seconds,
            max_output_tokens: result.role.max_output_tokens,
          });
          setRoles((current) =>
            current.map((role) =>
              role.role_id === roleId ? allowed : role,
            ),
          );
          toast.success("Model tested and allowed", allowed.label);
          refreshQuality();
        } catch (error) {
          toast.error(
            "Model passed but could not be allowed",
            errorMessage(error),
          );
        }
      } else {
        toast.error("Model test failed", modelTestFailureMessage(result.error_code));
      }
    } catch (error) {
      const message = errorMessage(error);
      setModelTestErrors((current) => ({ ...current, [roleId]: message }));
      toast.error("Model test could not run", message);
    } finally {
      setBusyItem(null);
    }
  }

  async function startQualityEvaluation(evaluation: ModelQualityEvaluation) {
    setSelectedTestRoleId(evaluation.role_id);
    setTestConsoleOpen(true);
    const isMusicTagging = evaluation.role_id === "music_tagger";
    const isTagCleanup = evaluation.role_id === "tag_cleanup";
    const isEqAssistance = evaluation.role_id === "eq_assistant";
    const confirmed = await confirmDialog({
      title: isMusicTagging
        ? "Run mood tagging model quality check?"
        : isTagCleanup
          ? "Run mood-tag cleanup model quality check?"
          : isEqAssistance
            ? "Run EQ assistant model quality check?"
            : "Run playlist model quality check?",
      body:
        `The provider will receive fixed synthetic ${
          isMusicTagging
            ? "music metadata cases"
            : isTagCleanup
              ? "tag-catalog cleanup cases"
              : isEqAssistance
                ? "EQ drafting scenarios"
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

  async function retestFailedScenarios(evaluation: ModelQualityEvaluation) {
    setSelectedTestRoleId(evaluation.role_id);
    setTestConsoleOpen(true);
    const confirmed = await confirmDialog({
      title: "Recheck failed mood-tagging scenarios?",
      body:
        "Only the failed scenarios from the last complete result will be sent again. " +
        "The merged report is diagnostic only. Run the complete suite before certification can change.",
      confirmLabel: "Recheck failures",
      tone: "primary",
    });
    if (!confirmed) return;
    setBusyItem(`evaluation-retest:${evaluation.evaluation_id}`);
    try {
      const job = await assistantProvidersApi.retestFailedScenarios(
        evaluation.role_id,
        evaluation.evaluation_id,
      );
      setQualityHistory((current) => [
        job,
        ...current.filter((item) => item.id !== job.id),
      ]);
      toast.success(
        "Failed scenarios queued",
        "Only the failed cases will call the provider; this will not change certification.",
      );
      refreshQuality();
    } catch (error) {
      toast.error("Failed scenarios could not be queued", errorMessage(error));
    } finally {
      setBusyItem(null);
    }
  }

  async function cancelQualityEvaluation(jobId: string) {
    const roleId = qualityHistory.find((job) => job.id === jobId)?.parameters
      .role_id;
    if (typeof roleId === "string") setSelectedTestRoleId(roleId);
    setTestConsoleOpen(true);
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
  const frameworkStatus = status;

  function renderRoleCard(role: ModelRole) {
    const evaluation = qualityEvaluations.find(
      (item) => item.role_id === role.role_id,
    );
    const evaluationHistory = qualityHistory.filter(
      (job) =>
        job.parameters.role_id === role.role_id &&
        job.parameters.evaluation_id === evaluation?.evaluation_id,
    );
    return (
      <ModelRoleCard
        key={role.role_id}
        role={role}
        connections={connections}
        adapters={frameworkStatus.adapters}
        capabilities={frameworkStatus.capabilities}
        credentialStorageReady={frameworkStatus.credential_storage_ready}
        busy={busyItem === `role:${role.role_id}`}
        testing={busyItem === `role-test:${role.role_id}`}
        qualityEvaluation={evaluation}
        qualityHistory={evaluationHistory}
        qualityLoading={qualityLoading}
        qualityActionBusy={busyItem?.startsWith("evaluation") === true}
        onSave={saveRole}
        onTest={testRole}
        onStartQuality={(item) => void startQualityEvaluation(item)}
        onCancelQuality={(jobId) => void cancelQualityEvaluation(jobId)}
        onViewTestLog={() => {
          setSelectedTestRoleId(role.role_id);
          setTestConsoleOpen(true);
        }}
        onRemove={removeRole}
      />
    );
  }

  const tagRoles = roles.filter((role) =>
    ["music_tagger", "tag_cleanup"].includes(role.role_id),
  );
  const standaloneRoles = roles.filter(
    (role) => !["music_tagger", "tag_cleanup"].includes(role.role_id),
  );

  return (
    <div className="assistant-provider-view">
      <header className="assistant-page-header">
        <div>
          <h1>Models and providers</h1>
          <p>Connect providers, then assign and test a model for each task.</p>
        </div>
        <div className="assistant-page-tools">
          <AssistantInfoPopover label="Privacy and local tools" title="Setup stays synthetic">
            <p>
              Verification and quality checks use provider metadata or fixed test
              inputs. They do not send songs, audio, filesystem paths, or live
              library tags.
            </p>
            <p>
              Local tools remain active. Real model requests show their own provider
              boundary before anything is sent.
            </p>
          </AssistantInfoPopover>
        </div>
      </header>

      <details
        className="surface-card assistant-setup-guide assistant-provider-onboarding"
        open={setupHubOpen}
        onToggle={(event) => setSetupHubOpen(event.currentTarget.open)}
      >
        <summary>
          <span>Add provider connection</span>
          <small>Setup guide · address · encrypted API key</small>
        </summary>
        <div className="assistant-provider-onboarding-body">
          <div className="assistant-provider-onboarding-guide">
            <h2>How setup works</h2>
            <ol className="assistant-provider-path" aria-label="Connection setup steps">
              <li><span>1</span><strong>Connect</strong><p>Save the key.</p></li>
              <li><span>2</span><strong>Verify</strong><p>Load models.</p></li>
              <li><span>3</span><strong>Assign</strong><p>Pick per task.</p></li>
              <li><span>4</span><strong>Test</strong><p>Check format.</p></li>
              <li><span>5</span><strong>Evaluate</strong><p>Check quality.</p></li>
            </ol>
          </div>
          {status.credential_storage_ready ? (
            <form
              className="assistant-provider-create"
              onSubmit={(event) => void createConnection(event)}
            >
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
                  {status.adapters.map((adapter) => (
                    <option key={adapter.id} value={adapter.id}>{adapter.label}</option>
                  ))}
                </select>
                <small className="field-hint">
                  {status.adapters.find((adapter) => adapter.id === adapterId)
                    ?.description}
                </small>
              </label>
              <label className="field assistant-provider-address-field">
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
              <label className="field assistant-provider-key-field">
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
                  busyItem !== null || !name.trim() || !adapterId ||
                  !baseUrl.trim() || !apiKey.trim()
                }
              >
                {busyItem === "create" ? "Saving…" : "Save connection"}
              </button>
            </form>
          ) : (
            <p className="assistant-provider-onboarding-locked">
              Prepare encrypted key storage below before adding a connection.
            </p>
          )}
        </div>
      </details>

      <section className="assistant-provider-section">
        <div className="assistant-section-heading">
          <div>
            <h2>Provider connections</h2>
            {connections.length === 0 ? (
              <p>
                Keys are encrypted by the server and never shown again. Use names to
                distinguish credentials, billing scopes, or model services.
              </p>
            ) : null}
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
          <div className="assistant-provider-card-list">
            {connections.length === 0 ? (
              status.credential_storage_ready ? null : (
                <div className="surface-card assistant-provider-empty">
                  <h3>No provider connections yet</h3>
                  <p>
                    Secure key storage must be ready before a provider can be added.
                  </p>
                </div>
              )
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
          </div>
          <span>
            {
              roles.filter(
                (role) =>
                  role.effective_enabled &&
                  qualityEvaluations.some(
                    (evaluation) =>
                      evaluation.role_id === role.role_id &&
                      evaluation.status === "passed",
                  ),
              ).length
            }{" "}
            ready
          </span>
        </div>
        {connections.length === 0 ? (
          <div className="surface-card assistant-provider-empty">
            <h3>Connections come first</h3>
            <p>Save and verify a provider above before assigning model tasks.</p>
          </div>
        ) : (
          <div className="assistant-role-grid">
            {tagRoles.length > 0 ? (
              <section className="assistant-role-family assistant-tag-role-family">
                <div className="assistant-role-family-heading">
                  <div>
                    <p className="assistant-eyebrow">Shared controlled vocabulary</p>
                    <h3>Tag intelligence</h3>
                    <p>
                      Tagging chooses canonical IDs; cleanup maps existing names back
                      to those same IDs. Keep separate models when speed and semantic
                      depth need different settings.
                    </p>
                  </div>
                  <span>one vocabulary · two tasks</span>
                </div>
                <div className="assistant-role-family-grid">
                  {tagRoles.map(renderRoleCard)}
                </div>
              </section>
            ) : null}
            {standaloneRoles.map(renderRoleCard)}
          </div>
        )}
        {connections.length > 0 ? (
          <ModelTestConsole
            open={testConsoleOpen}
            roles={roles}
            evaluations={qualityEvaluations}
            history={qualityHistory}
            connections={connections}
            adapters={status.adapters}
            selectedRoleId={selectedTestRoleId}
            testingRoleId={
              busyItem?.startsWith("role-test:") === true
                ? busyItem.slice("role-test:".length)
                : null
            }
            modelTestResults={modelTestResults}
            modelTestErrors={modelTestErrors}
            qualityLoading={qualityLoading}
            qualityLoadError={qualityLoadError}
            onSelectRole={setSelectedTestRoleId}
            onRetryQuality={refreshQuality}
            onRetestFailed={(item) => void retestFailedScenarios(item)}
            qualityActionBusy={busyItem?.startsWith("evaluation") === true}
            onOpenChange={setTestConsoleOpen}
          />
        ) : null}
      </section>

    </div>
  );
}
