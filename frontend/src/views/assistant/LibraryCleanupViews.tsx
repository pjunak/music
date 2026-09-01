import { useEffect, useState } from "react";
import { Link, useLocation, useNavigate } from "react-router-dom";

import { CleanupHistoryPanel } from "@/components/CleanupHistoryPanel";
import { CleanupWorkflow } from "@/components/CleanupDialog";
import { confirmDialog } from "@/components/confirmDialog";
import type { CleanupSource } from "@/core/api";
import { cleanupApi } from "@/core/api";
import type { ModelRole, ProviderFrameworkStatus } from "@/core/assistantProvidersApi";
import { assistantProvidersApi } from "@/core/assistantProvidersApi";
import { toast } from "@/core/toast";

import { CredentialStorageCard } from "./CredentialStorageCard";

interface CleanupRouteState {
  cleanupScope?: {
    path?: string;
    trackIds?: number[];
  };
}

function CleanupEvidenceRail() {
  return (
    <ol className="cleanup-evidence-rail" aria-label="Cleanup evidence order">
      <li className="is-ready">
        <span>1</span>
        <div>
          <strong>Local rules</strong>
          <small>Files, tags, and folder structure</small>
        </div>
        <em>authoritative</em>
      </li>
      <li>
        <span>2</span>
        <div>
          <strong>Catalog sources</strong>
          <small>Online identity and metadata evidence</small>
        </div>
        <Link to="../sources">configure</Link>
      </li>
      <li>
        <span>3</span>
        <div>
          <strong>AI assistance</strong>
          <small>Optional ambiguity review</small>
        </div>
        <Link to="../model">inspect</Link>
      </li>
    </ol>
  );
}

export function LibraryCleanupRunView() {
  const navigate = useNavigate();
  const location = useLocation();
  const state = location.state as CleanupRouteState | null;
  const path = state?.cleanupScope?.path ?? "";
  const checkedIds = state?.cleanupScope?.trackIds ?? [];

  return (
    <div className="assistant-cleanup-view">
      <header className="assistant-page-header">
        <div>
          <p className="assistant-eyebrow">Library cleanup</p>
          <h1>Repair the library from evidence</h1>
          <p>
            Start with local, deterministic fixes. Online catalogs can add identity evidence, while
            the future model role remains visibly reserved. Every proposed write stays optional.
          </p>
        </div>
        <Link className="btn-link" to="../history">
          History &amp; rollback
        </Link>
      </header>

      <CleanupEvidenceRail />

      <CleanupWorkflow
        path={path}
        checkedIds={checkedIds}
        onClose={() => navigate("/library")}
        onOpenHistory={() => navigate("../history")}
        onApplied={() => undefined}
      />
    </div>
  );
}

export function LibraryCleanupHistoryView() {
  return (
    <div className="assistant-cleanup-view assistant-cleanup-history-view">
      <header className="assistant-page-header">
        <div>
          <p className="assistant-eyebrow">Library cleanup</p>
          <h1>History &amp; rollback</h1>
          <p>
            Every applied cleanup run keeps its original values. Download a journal for
            safekeeping or restore changes that still match the recorded result.
          </p>
        </div>
        <Link className="btn-link" to="../run">
          Start cleanup
        </Link>
      </header>

      <section className="surface-card cleanup-history-card" aria-label="Cleanup journals">
        <CleanupHistoryPanel />
      </section>
    </div>
  );
}

function capabilityLabel(capability: string): string {
  switch (capability) {
    case "artist_name_verification":
      return "artist names";
    case "album_name_verification":
      return "album names";
    case "recording_identity":
      return "recording identity";
    case "canonical_metadata":
      return "canonical metadata";
    case "acoustic_fingerprint_identity":
      return "fingerprint fallback";
    case "community_tag_evidence":
      return "community tag evidence";
    default:
      return capability.replaceAll("_", " ");
  }
}

export function LibraryCleanupSourcesView() {
  const [sources, setSources] = useState<CleanupSource[]>([]);
  const [loading, setLoading] = useState(true);
  const [loadError, setLoadError] = useState<string | null>(null);
  const [savingId, setSavingId] = useState<string | null>(null);
  const [credentialStatus, setCredentialStatus] = useState<ProviderFrameworkStatus | null>(null);
  const [credentialValues, setCredentialValues] = useState<Record<string, string>>({});
  const [storageInitializing, setStorageInitializing] = useState(false);
  const [refreshKey, setRefreshKey] = useState(0);

  useEffect(() => {
    let disposed = false;
    setLoading(true);
    Promise.all([
      cleanupApi.sources(),
      assistantProvidersApi.getStatus().catch(() => null),
    ])
      .then(([nextSources, nextCredentialStatus]) => {
        if (disposed) return;
        setSources(nextSources);
        setCredentialStatus(nextCredentialStatus);
        setLoadError(
          nextCredentialStatus === null
            ? "Encrypted key-storage status is unavailable. Existing source switches still work."
            : null,
        );
      })
      .catch((error: unknown) => {
        if (!disposed) {
          setLoadError(error instanceof Error ? error.message : "Source settings are unavailable.");
        }
      })
      .finally(() => {
        if (!disposed) setLoading(false);
      });
    return () => {
      disposed = true;
    };
  }, [refreshKey]);

  async function setEnabled(source: CleanupSource, enabled: boolean) {
    setSavingId(source.id);
    try {
      const updated = await cleanupApi.updateSource(source.id, enabled);
      setSources((current) => current.map((item) => (item.id === updated.id ? updated : item)));
      toast.success(`${updated.label} ${updated.enabled ? "enabled" : "disabled"}`);
    } catch (error) {
      toast.error(
        "Source setting was not saved",
        error instanceof Error ? error.message : undefined,
      );
    } finally {
      setSavingId(null);
    }
  }

  async function initializeCredentialStorage() {
    setStorageInitializing(true);
    try {
      const status = await assistantProvidersApi.initializeCredentialStorage();
      setCredentialStatus(status);
      setRefreshKey((value) => value + 1);
      toast.success("Encrypted storage initialized", "Catalog API keys can now be saved here.");
    } catch (error) {
      toast.error(
        "Encrypted storage could not be initialized",
        error instanceof Error ? error.message : undefined,
      );
    } finally {
      setStorageInitializing(false);
    }
  }

  async function saveCredential(source: CleanupSource) {
    const apiKey = credentialValues[source.id]?.trim() ?? "";
    if (!apiKey) return;
    setSavingId(source.id);
    try {
      const updated = await cleanupApi.saveSourceCredential(source.id, apiKey);
      setSources((current) => current.map((item) => (item.id === updated.id ? updated : item)));
      setCredentialValues((current) => ({ ...current, [source.id]: "" }));
      toast.success(
        `${updated.label} API key ${source.credential_saved ? "replaced" : "saved"}`,
        "The key is encrypted on the server and will not be shown again.",
      );
    } catch (error) {
      toast.error(
        "API key was not saved",
        error instanceof Error ? error.message : undefined,
      );
    } finally {
      setSavingId(null);
    }
  }

  async function deleteCredential(source: CleanupSource) {
    const confirmed = await confirmDialog({
      title: `Remove the saved ${source.label} key?`,
      body:
        source.credential_source === "saved" && source.configuration_hint?.includes("fallback")
          ? "The encrypted key will be deleted. If a server-environment fallback exists, it becomes active again; otherwise this source will need setup."
          : "The encrypted key will be deleted and this source will need setup before it can be used again.",
      confirmLabel: "Remove API key",
      tone: "danger",
    });
    if (!confirmed) return;
    setSavingId(source.id);
    try {
      const updated = await cleanupApi.deleteSourceCredential(source.id);
      setSources((current) => current.map((item) => (item.id === updated.id ? updated : item)));
      toast.success(`${updated.label} saved API key removed`);
    } catch (error) {
      toast.error(
        "API key was not removed",
        error instanceof Error ? error.message : undefined,
      );
    } finally {
      setSavingId(null);
    }
  }

  return (
    <div className="assistant-cleanup-view assistant-cleanup-sources-view">
      <header className="assistant-page-header">
        <div>
          <p className="assistant-eyebrow">Library cleanup</p>
          <h1>Catalog sources</h1>
          <p>
            Choose which explicit, maintained connectors may add evidence to a cleanup run.
            Local analysis still works when every online source is off.
          </p>
        </div>
        <Link className="btn-link" to="../run">
          Back to cleanup
        </Link>
      </header>

      {loadError !== null ? (
        <div className="assistant-analysis-error cleanup-source-error" role="alert">
          <span>{loadError}</span>
          <button type="button" onClick={() => setRefreshKey((value) => value + 1)}>
            Retry
          </button>
        </div>
      ) : null}

      <section className="surface-card cleanup-source-contract">
        <div>
          <p className="assistant-eyebrow">Connector boundary</p>
          <h2>Sources are adapters, not arbitrary websites</h2>
        </div>
        <p className="muted small">
          Each source must define its fields, rate limits, attribution, and confidence mapping.
          Keys saved here use the same encrypted server vault as model-provider credentials and
          are never returned to the browser. Environment keys remain deployment-managed fallbacks.
        </p>
      </section>

      {credentialStatus !== null ? (
        <CredentialStorageCard
          status={credentialStatus}
          busy={storageInitializing}
          onInitialize={initializeCredentialStorage}
        />
      ) : null}

      <div className="cleanup-source-list" aria-busy={loading}>
        {loading ? <p className="muted small">Loading source settings…</p> : null}
        {sources.map((source) => (
          <section key={source.id} className="surface-card cleanup-source-card">
            <div className="cleanup-source-heading">
              <div>
                <div className="cleanup-source-title-row">
                  <h2>{source.label}</h2>
                  <span
                    className={`badge${source.enabled && source.available ? " badge-ok" : ""}`}
                  >
                    {!source.available ? "needs setup" : source.enabled ? "active" : "off"}
                  </span>
                </div>
                <p>{source.description}</p>
              </div>
              <label className="cleanup-source-toggle">
                <input
                  type="checkbox"
                  checked={source.enabled}
                  disabled={savingId !== null || (!source.available && !source.enabled)}
                  onChange={(event) => void setEnabled(source, event.target.checked)}
                />
                <span>{savingId === source.id ? "Saving…" : "Use in cleanup"}</span>
              </label>
            </div>
            <dl className="cleanup-source-details">
              <div>
                <dt>Evidence</dt>
                <dd>{source.capabilities.map(capabilityLabel).join(", ")}</dd>
              </div>
              <div>
                <dt>API access</dt>
                <dd>
                  {source.credential_kind === null
                    ? "No API key required"
                    : source.credential_source === "saved"
                      ? `${source.credential_kind} saved${source.key_hint ? ` · ${source.key_hint}` : ""}`
                      : source.credential_source === "environment"
                        ? `${source.credential_kind} from server environment`
                      : `${source.credential_kind} required`}
                  {source.configuration_hint !== null ? (
                    <small>{source.configuration_hint}</small>
                  ) : null}
                </dd>
              </div>
              <div>
                <dt>Writes</dt>
                <dd>Never direct; suggestions return to review</dd>
              </div>
            </dl>
            {source.credential_kind !== null ? (
              <form
                className="cleanup-source-credential-form"
                onSubmit={(event) => {
                  event.preventDefault();
                  void saveCredential(source);
                }}
              >
                <label className="field">
                  <span className="field-label">
                    {source.credential_saved ? `Replace ${source.label} API key` : `${source.label} API key`}
                  </span>
                  <input
                    type="password"
                    value={credentialValues[source.id] ?? ""}
                    maxLength={4096}
                    autoComplete="new-password"
                    disabled={savingId !== null || credentialStatus?.credential_storage_ready !== true}
                    placeholder={source.credential_saved ? "Enter replacement key" : "Enter API key"}
                    onChange={(event) =>
                      setCredentialValues((current) => ({
                        ...current,
                        [source.id]: event.target.value,
                      }))
                    }
                  />
                </label>
                <div className="cleanup-source-credential-actions">
                  <button
                    className="btn-primary"
                    type="submit"
                    disabled={
                      savingId !== null ||
                      credentialStatus?.credential_storage_ready !== true ||
                      !(credentialValues[source.id]?.trim() ?? "")
                    }
                  >
                    {savingId === source.id
                      ? "Saving…"
                      : source.credential_saved
                        ? "Replace saved key"
                        : "Save API key"}
                  </button>
                  {source.credential_saved ? (
                    <button
                      className="btn-ghost"
                      type="button"
                      disabled={savingId !== null}
                      onClick={() => void deleteCredential(source)}
                    >
                      Remove saved key
                    </button>
                  ) : null}
                </div>
                {credentialStatus?.credential_storage_ready !== true ? (
                  <small className="field-hint">
                    Prepare encrypted key storage above before saving this key.
                  </small>
                ) : (
                  <small className="field-hint">
                    Saving a new value replaces the encrypted key immediately. The existing value
                    is never loaded into this field.
                  </small>
                )}
              </form>
            ) : null}
            {source.unavailable_reason !== null ? (
              <p className="cleanup-source-requirement muted small">
                {source.unavailable_reason}
              </p>
            ) : null}
          </section>
        ))}
      </div>
    </div>
  );
}

export function LibraryCleanupModelView() {
  const [role, setRole] = useState<ModelRole | null>(null);
  const [loading, setLoading] = useState(true);
  const [loadError, setLoadError] = useState<string | null>(null);
  const [refreshKey, setRefreshKey] = useState(0);

  useEffect(() => {
    let disposed = false;
    setLoading(true);
    assistantProvidersApi
      .listRoles()
      .then((roles) => {
        if (disposed) return;
        setRole(roles.find((item) => item.role_id === "library_cleanup") ?? null);
        setLoadError(null);
      })
      .catch((error: unknown) => {
        if (!disposed) {
          setLoadError(error instanceof Error ? error.message : "Model role status is unavailable.");
        }
      })
      .finally(() => {
        if (!disposed) setLoading(false);
      });
    return () => {
      disposed = true;
    };
  }, [refreshKey]);

  return (
    <div className="assistant-cleanup-view assistant-cleanup-model-view">
      <header className="assistant-page-header">
        <div>
          <p className="assistant-eyebrow">Library cleanup</p>
          <h1>AI assistance</h1>
          <p>
            This task-specific role is kept beside the cleanup workflow. Provider credentials
            and reusable connections remain in global Assistant setup.
          </p>
        </div>
        <Link className="btn-link" to="/assistant/settings/models">
          Manage provider connections
        </Link>
      </header>

      {loadError !== null ? (
        <div className="assistant-analysis-error cleanup-model-error" role="alert">
          <span>{loadError}</span>
          <button type="button" onClick={() => setRefreshKey((value) => value + 1)}>
            Retry
          </button>
        </div>
      ) : null}

      <section className="surface-card cleanup-model-card" aria-busy={loading}>
        {loading ? <p className="muted small">Loading model role…</p> : null}
        {!loading && role === null && loadError === null ? (
          <div role="alert">
            <h2>Library cleanup role is unavailable</h2>
            <p className="muted small">The server did not advertise the expected task role.</p>
          </div>
        ) : null}
        {role !== null ? (
          <>
            <div className="cleanup-model-heading">
              <div>
                <div className="cleanup-source-title-row">
                  <h2>{role.label}</h2>
                  <span className={`badge${role.configuration_available ? " badge-ok" : ""}`}>
                    {role.configuration_available ? "configurable" : "planned"}
                  </span>
                </div>
                <p>{role.description}</p>
              </div>
              <span className="cleanup-model-role-id">{role.role_id}</span>
            </div>

            <div className="cleanup-model-boundary">
              <div>
                <strong>Local cleanup stays authoritative</strong>
                <p>
                  A future model pass may compare ambiguous candidates and explain its choice.
                  It will not rename files or write tags without the same review and journal used
                  by local rules.
                </p>
              </div>
              <div>
                <strong>Unavailable until its contract is testable</strong>
                <p>
                  Model selection remains locked until the cleanup response schema, bounded
                  evidence input, and quality suite are implemented together.
                </p>
              </div>
            </div>

            <dl className="cleanup-source-details cleanup-model-details">
              <div>
                <dt>Required capability</dt>
                <dd>{role.required_capability_ids.join(", ") || "none"}</dd>
              </div>
              <div>
                <dt>Connection</dt>
                <dd>{role.connection_name ?? "Not assigned"}</dd>
              </div>
              <div>
                <dt>Runtime state</dt>
                <dd>{role.effective_enabled ? "enabled" : "not active"}</dd>
              </div>
            </dl>
          </>
        ) : null}
      </section>
    </div>
  );
}
