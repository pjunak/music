import { api, type BackgroundJob } from "@/core/api";

export type ProviderVerificationStatus = "never" | "verified" | "failed";
export type ModelConformanceStatus = "never" | "passed" | "failed";
export type ModelQualityEvaluationStatus =
  | "never"
  | "passed"
  | "failed"
  | "stale";

export interface ProviderCapability {
  id: string;
  label: string;
  description: string;
}

export interface ProviderAdapter {
  id: string;
  label: string;
  description: string;
  capability_ids: string[];
}

export interface ModelRoleDefinition {
  id: string;
  label: string;
  description: string;
  required_capability_ids: string[];
  configuration_available: boolean;
}

export interface ProviderFrameworkStatus {
  credential_storage_ready: boolean;
  credential_storage_error: string | null;
  credential_storage_source: "environment" | "file" | null;
  credential_storage_key_id: string | null;
  credential_storage_key_file_path: string | null;
  credential_storage_host_directory_hint: string | null;
  credential_storage_can_initialize: boolean;
  credential_storage_initialization_error: string | null;
  capabilities: ProviderCapability[];
  adapters: ProviderAdapter[];
  roles: ModelRoleDefinition[];
}

export interface ProviderCredentialStorageResetResult {
  deleted_credentials: number;
  master_key_removed: boolean;
  master_key_removal_error: string | null;
  status: ProviderFrameworkStatus;
}

export interface ProviderConnection {
  id: string;
  name: string;
  adapter_id: string;
  base_url: string;
  credential_saved: boolean;
  key_hint: string | null;
  allow_private_network: boolean;
  verification_status: ProviderVerificationStatus;
  verification_error_code: string | null;
  verified_models: string[];
  verified_capability_ids: string[];
  last_verified_at: string | null;
  created_at: string;
  updated_at: string;
}

export interface ProviderConnectionCreate {
  name: string;
  adapter_id: string;
  base_url: string;
  api_key: string;
  allow_private_network: boolean;
}

export interface ProviderConnectionUpdate {
  name?: string;
  adapter_id?: string;
  base_url?: string;
  api_key?: string;
  allow_private_network?: boolean;
}

export interface ProviderVerification {
  connection: ProviderConnection;
  verified: boolean;
  error_code: string | null;
  models: string[];
}

export interface ModelRole {
  role_id: string;
  label: string;
  description: string;
  required_capability_ids: string[];
  configuration_available: boolean;
  connection_id: string | null;
  connection_name: string | null;
  model_id: string;
  enabled: boolean;
  effective_enabled: boolean;
  timeout_seconds: number;
  max_output_tokens: number;
  verification_status: ProviderVerificationStatus | null;
  conformance_status: ModelConformanceStatus;
  conformance_error_code: string | null;
  last_conformance_at: string | null;
  updated_at: string | null;
}

export interface ModelConformance {
  role: ModelRole;
  passed: boolean;
  error_code: string | null;
}

export interface ModelQualityEvaluation {
  evaluation_id: string;
  role_id: string;
  label: string;
  description: string;
  status: ModelQualityEvaluationStatus;
  suite_id: string;
  passed_cases: number;
  total_cases: number;
  last_job_id: string | null;
  last_evaluated_at: string | null;
}

export interface ModelRoleUpdate {
  connection_id: string;
  model_id: string;
  enabled: boolean;
  timeout_seconds?: number;
  max_output_tokens?: number;
}

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
  deleteRole: (roleId: string) =>
    api.delete<void>(
      `/api/assistant/providers/roles/${encodeURIComponent(roleId)}`,
    ),
};
