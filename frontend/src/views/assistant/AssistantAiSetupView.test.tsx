import { render, screen, waitFor } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { beforeEach, describe, expect, it, vi } from "vitest";

import type * as ProviderApiModule from "@/core/assistantProvidersApi";
import type {
  ModelRole,
  ProviderConnection,
  ProviderFrameworkStatus,
} from "@/core/assistantProvidersApi";

vi.mock("@/core/assistantProvidersApi", async (importActual) => {
  const actual = await importActual<typeof ProviderApiModule>();
  return {
    ...actual,
    assistantProvidersApi: {
      getStatus: vi.fn(),
      listConnections: vi.fn(),
      createConnection: vi.fn(),
      updateConnection: vi.fn(),
      deleteConnection: vi.fn(),
      verifyConnection: vi.fn(),
      listRoles: vi.fn(),
      updateRole: vi.fn(),
      deleteRole: vi.fn(),
    },
  };
});

vi.mock("@/components/confirmDialog", () => ({ confirmDialog: vi.fn() }));

vi.mock("@/core/toast", () => ({
  toast: {
    success: vi.fn(),
    error: vi.fn(),
  },
}));

import { confirmDialog } from "@/components/confirmDialog";
import { assistantProvidersApi } from "@/core/assistantProvidersApi";
import { toast } from "@/core/toast";

import { AssistantAiSetupView } from "./AssistantAiSetupView";

const frameworkStatus: ProviderFrameworkStatus = {
  credential_storage_ready: true,
  credential_storage_error: null,
  adapters: [
    {
      id: "openai-compatible/v1",
      label: "OpenAI-compatible API",
      description: "Connects to a provider with a compatible models endpoint.",
    },
  ],
  roles: [
    {
      id: "playlist_planner",
      label: "Playlist planner",
      description: "Plans candidate playlists from reviewed library information.",
    },
  ],
};

const connection: ProviderConnection = {
  id: "connection-1",
  name: "Hosted models",
  adapter_id: "openai-compatible/v1",
  base_url: "https://models.example/v1",
  key_hint: "••••1234",
  allow_private_network: false,
  verification_status: "verified",
  verification_error_code: null,
  verified_models: ["planner-large", "tagger-small"],
  last_verified_at: "2026-08-19T10:00:00Z",
  created_at: "2026-08-19T09:00:00Z",
  updated_at: "2026-08-19T10:00:00Z",
};

const role: ModelRole = {
  role_id: "playlist_planner",
  label: "Playlist planner",
  description: "Plans candidate playlists from reviewed library information.",
  connection_id: null,
  connection_name: null,
  model_id: "",
  enabled: false,
  effective_enabled: false,
  timeout_seconds: 30,
  max_output_tokens: 2000,
  verification_status: null,
  updated_at: null,
};

beforeEach(() => {
  vi.clearAllMocks();
  vi.mocked(assistantProvidersApi.getStatus).mockResolvedValue(frameworkStatus);
  vi.mocked(assistantProvidersApi.listConnections).mockResolvedValue([]);
  vi.mocked(assistantProvidersApi.listRoles).mockResolvedValue([role]);
  vi.mocked(confirmDialog).mockResolvedValue(true);
});

describe("AssistantAiSetupView", () => {
  it("saves a provider connection and clears the API key from the form", async () => {
    const user = userEvent.setup();
    vi.mocked(assistantProvidersApi.createConnection).mockResolvedValue({
      ...connection,
      verification_status: "never",
      verified_models: [],
      last_verified_at: null,
    });
    render(<AssistantAiSetupView />);

    await screen.findByRole("heading", { name: "AI connections" });
    await user.type(screen.getByLabelText("Connection name"), "Hosted models");
    await user.type(
      screen.getByLabelText("Provider address"),
      "https://models.example/v1",
    );
    await user.type(screen.getByLabelText("API key"), "secret-key-1234");
    await user.click(screen.getByRole("button", { name: "Save connection" }));

    await waitFor(() =>
      expect(assistantProvidersApi.createConnection).toHaveBeenCalledWith({
        name: "Hosted models",
        adapter_id: "openai-compatible/v1",
        base_url: "https://models.example/v1",
        api_key: "secret-key-1234",
        allow_private_network: false,
      }),
    );
    expect(screen.getByLabelText("API key")).toHaveValue("");
    expect(
      await screen.findByRole("heading", { name: "Hosted models" }),
    ).toBeInTheDocument();
    expect(toast.success).toHaveBeenCalledWith(
      "Connection saved",
      "Verify it before assigning any model tasks.",
    );
  });

  it("verifies a saved connection explicitly", async () => {
    const user = userEvent.setup();
    vi.mocked(assistantProvidersApi.listConnections).mockResolvedValue([
      {
        ...connection,
        verification_status: "never",
        verified_models: [],
        last_verified_at: null,
      },
    ]);
    vi.mocked(assistantProvidersApi.verifyConnection).mockResolvedValue({
      connection,
      verified: true,
      error_code: null,
      models: connection.verified_models,
    });
    render(<AssistantAiSetupView />);

    await user.click(
      await screen.findByRole("button", { name: "Verify connection" }),
    );

    await waitFor(() =>
      expect(assistantProvidersApi.verifyConnection).toHaveBeenCalledWith(
        "connection-1",
      ),
    );
    expect(toast.success).toHaveBeenCalledWith(
      "Connection verified",
      "2 models available.",
    );
  });

  it("assigns a verified connection and model to one task", async () => {
    const user = userEvent.setup();
    vi.mocked(assistantProvidersApi.listConnections).mockResolvedValue([connection]);
    vi.mocked(assistantProvidersApi.updateRole).mockResolvedValue({
      ...role,
      connection_id: connection.id,
      connection_name: connection.name,
      model_id: "planner-large",
      enabled: true,
      effective_enabled: true,
      verification_status: "verified",
    });
    render(<AssistantAiSetupView />);

    await user.selectOptions(await screen.findByLabelText("Connection"), connection.id);
    await user.type(screen.getByLabelText("Model"), "planner-large");
    await user.click(screen.getByLabelText("Allow this model for this task"));
    await user.click(screen.getByRole("button", { name: "Save task" }));

    await waitFor(() =>
      expect(assistantProvidersApi.updateRole).toHaveBeenCalledWith(
        "playlist_planner",
        {
          connection_id: "connection-1",
          model_id: "planner-large",
          enabled: true,
          timeout_seconds: 30,
          max_output_tokens: 2000,
        },
      ),
    );
  });

  it("keeps local tools available when encrypted storage is not configured", async () => {
    vi.mocked(assistantProvidersApi.getStatus).mockResolvedValue({
      ...frameworkStatus,
      credential_storage_ready: false,
      credential_storage_error: "master_key_not_configured",
    });
    render(<AssistantAiSetupView />);

    expect(
      await screen.findByText("Encrypted key storage needs one server setting"),
    ).toBeInTheDocument();
    expect(screen.getByText(/Local analysis and playlist building continue/)).toBeInTheDocument();
    expect(screen.queryByLabelText("API key")).not.toBeInTheDocument();
  });
});
