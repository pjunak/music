import { api, type BackgroundJob } from "@/core/api";
import type * as Http from "@/generated/http";

export type ProviderVerificationStatus = "never" | "verified" | "failed";
export type ModelConformanceStatus = "never" | "passed" | "failed";
export type ModelThinkingMode = "provider_default" | "enabled" | "disabled" | "low" | "medium" | "high" | "xhigh" | "max";
export type ModelQualityEvaluationStatus =
  | "never"
  | "passed"
  | "failed"
  | "stale";

export type ProviderCapability = Http.ProviderCapabilityOut;

export type ProviderAdapter = Http.ProviderAdapterOut;

export type ProviderModelProfile = Http.ModelProfileResponse;

export type ModelRoleDefinition = Http.ModelRoleDefinitionOut;

export type ProviderFrameworkStatus = Http.ProviderFrameworkStatusOut;

export type ProviderCredentialStorageResetResult = Http.ProviderCredentialStorageResetOut;

export type ProviderConnection = Http.ProviderConnectionOut;

export type ProviderConnectionCreate = Http.ProviderConnectionCreate;

export type ProviderConnectionUpdate = Http.ProviderConnectionUpdate;

export type ProviderVerification = Http.ProviderVerificationOut;

export type ModelRole = Http.ModelRoleOut;

export type ModelConformance = Http.ModelConformanceOut;

export type ModelQualityEvaluation = Http.ModelQualityEvaluationOut;

export type ModelRoleUpdate = Http.ModelRoleUpdate;

export const assistantProvidersApi = {
  getStatus: () =>
    api.get<ProviderFrameworkStatus>("/api/assistant/providers/status"),
  initializeCredentialStorage: () =>
    api.post<ProviderFrameworkStatus>(
      "/api/assistant/providers/credential-storage/initialize",
    ),
  resetCredentialStorage: (currentPassword: string) =>
    api.post<ProviderCredentialStorageResetResult>(
      "/api/assistant/providers/credential-storage/reset",
      { current_password: currentPassword },
    ),
  listConnections: () =>
    api.get<ProviderConnection[]>("/api/assistant/providers/connections"),
  createConnection: (payload: ProviderConnectionCreate) =>
    api.post<ProviderConnection>("/api/assistant/providers/connections", payload),
  updateConnection: (connectionId: string, payload: ProviderConnectionUpdate) =>
    api.put<ProviderConnection>(
      `/api/assistant/providers/connections/${encodeURIComponent(connectionId)}`,
      payload,
    ),
  deleteConnection: (connectionId: string) =>
    api.delete<void>(
      `/api/assistant/providers/connections/${encodeURIComponent(connectionId)}`,
    ),
  deleteConnectionCredential: (connectionId: string) =>
    api.delete<ProviderConnection>(
      `/api/assistant/providers/connections/${encodeURIComponent(connectionId)}/credential`,
    ),
  verifyConnection: (connectionId: string) =>
    api.post<ProviderVerification>(
      `/api/assistant/providers/connections/${encodeURIComponent(connectionId)}/verify`,
    ),
  listRoles: () => api.get<ModelRole[]>("/api/assistant/providers/roles"),
  updateRole: (roleId: string, payload: ModelRoleUpdate) =>
    api.put<ModelRole>(
      `/api/assistant/providers/roles/${encodeURIComponent(roleId)}`,
      payload,
    ),
  testRole: (roleId: string) =>
    api.post<ModelConformance>(
      `/api/assistant/providers/roles/${encodeURIComponent(roleId)}/test`,
    ),
  listRoleEvaluations: (roleId: string) =>
    api.get<ModelQualityEvaluation[]>(
      `/api/assistant/providers/roles/${encodeURIComponent(roleId)}/evaluations`,
    ),
  startRoleEvaluation: (roleId: string, evaluationId: string) =>
    api.post<BackgroundJob>(
      `/api/assistant/providers/roles/${encodeURIComponent(roleId)}/evaluations/${encodeURIComponent(evaluationId)}/jobs`,
    ),
  retestFailedScenarios: (roleId: string, evaluationId: string) =>
    api.post<BackgroundJob>(
      `/api/assistant/providers/roles/${encodeURIComponent(roleId)}/evaluations/${encodeURIComponent(evaluationId)}/failed-scenarios/jobs`,
    ),
  deleteRole: (roleId: string) =>
    api.delete<void>(
      `/api/assistant/providers/roles/${encodeURIComponent(roleId)}`,
    ),
};
