import { api } from "@/core/api";

export type ProviderVerificationStatus = "never" | "verified" | "failed";

export interface ProviderAdapter {
  id: string;
  label: string;
  description: string;
}

export interface ModelRoleDefinition {
  id: string;
  label: string;
  description: string;
}

export interface ProviderFrameworkStatus {
  credential_storage_ready: boolean;
  credential_storage_error: string | null;
  adapters: ProviderAdapter[];
  roles: ModelRoleDefinition[];
}

export interface ProviderConnection {
  id: string;
  name: string;
  adapter_id: string;
  base_url: string;
  key_hint: string;
  allow_private_network: boolean;
  verification_status: ProviderVerificationStatus;
  verification_error_code: string | null;
  verified_models: string[];
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
  connection_id: string | null;
  connection_name: string | null;
  model_id: string;
  enabled: boolean;
  effective_enabled: boolean;
  timeout_seconds: number;
  max_output_tokens: number;
  verification_status: ProviderVerificationStatus | null;
  updated_at: string | null;
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
  deleteRole: (roleId: string) =>
    api.delete<void>(
      `/api/assistant/providers/roles/${encodeURIComponent(roleId)}`,
    ),
};
