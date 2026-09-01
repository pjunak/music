import { render, screen, waitFor, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { beforeEach, describe, expect, it, vi } from "vitest";

import type * as ProviderApiModule from "@/core/assistantProvidersApi";
import type * as ApiModule from "@/core/api";
import type { BackgroundJob } from "@/core/api";
import type {
  ModelQualityEvaluation,
  ModelRole,
  ProviderConnection,
  ProviderFrameworkStatus,
} from "@/core/assistantProvidersApi";

const TEST_PROVIDER_API_KEY = ["test", "provider", "credential", "1234"].join("-");

vi.mock("@/core/api", async (importActual) => {
  const actual = await importActual<typeof ApiModule>();
  return {
    ...actual,
    jobsApi: {
      list: vi.fn(),
      get: vi.fn(),
      cancel: vi.fn(),
      retry: vi.fn(),
    },
  };
});

vi.mock("@/core/assistantProvidersApi", async (importActual) => {
  const actual = await importActual<typeof ProviderApiModule>();
  return {
    ...actual,
    assistantProvidersApi: {
      getStatus: vi.fn(),
      initializeCredentialStorage: vi.fn(),
      resetCredentialStorage: vi.fn(),
      listConnections: vi.fn(),
      createConnection: vi.fn(),
      updateConnection: vi.fn(),
      deleteConnection: vi.fn(),
      deleteConnectionCredential: vi.fn(),
      verifyConnection: vi.fn(),
      listRoles: vi.fn(),
      updateRole: vi.fn(),
      testRole: vi.fn(),
      listRoleEvaluations: vi.fn(),
      startRoleEvaluation: vi.fn(),
      retestFailedScenarios: vi.fn(),
      deleteRole: vi.fn(),
    },
  };
});

vi.mock("@/components/confirmDialog", () => ({ confirmDialog: vi.fn() }));
vi.mock("@/components/inputDialog", () => ({ inputDialog: vi.fn() }));

vi.mock("@/core/toast", () => ({
  toast: {
    success: vi.fn(),
    error: vi.fn(),
  },
}));

import { confirmDialog } from "@/components/confirmDialog";
import { inputDialog } from "@/components/inputDialog";
import { jobsApi } from "@/core/api";
import { assistantProvidersApi } from "@/core/assistantProvidersApi";
import { toast } from "@/core/toast";

import { AssistantAiSetupView } from "./AssistantAiSetupView";

const frameworkStatus: ProviderFrameworkStatus = {
  credential_storage_ready: true,
  credential_storage_error: null,
  credential_storage_source: "environment",
  credential_storage_key_id: "0123456789abcdef",
  credential_storage_key_file_path: null,
  credential_storage_host_directory_hint: null,
  credential_storage_can_initialize: false,
  credential_storage_initialization_error: "master_key_already_configured",
  capabilities: [
    {
      id: "structured-text/v1",
      label: "Structured text",
      description: "Sends text and receives a validated structured result.",
    },
    {
      id: "audio-input/v1",
      label: "Audio input",
      description: "Accepts bounded audio through a dedicated adapter.",
    },
  ],
  adapters: [
    {
      id: "openai-compatible/v1",
      label: "OpenAI-compatible API",
      description: "Connects to a provider with a compatible models endpoint.",
      capability_ids: ["structured-text/v1"],
    },
  ],
  roles: [
    {
      id: "playlist_planner",
      label: "Playlist planner",
      description: "Plans candidate playlists from reviewed library information.",
      required_capability_ids: ["structured-text/v1"],
      configuration_available: true,
    },
  ],
};

const connection: ProviderConnection = {
  id: "connection-1",
  name: "Hosted models",
  adapter_id: "openai-compatible/v1",
  base_url: "https://models.example/v1",
  credential_saved: true,
  key_hint: "••••1234",
  allow_private_network: false,
  verification_status: "verified",
  verification_error_code: null,
  verified_models: ["planner-large", "tagger-small"],
  verified_capability_ids: ["structured-text/v1"],
  last_verified_at: "2026-08-19T10:00:00Z",
  created_at: "2026-08-19T09:00:00Z",
  updated_at: "2026-08-19T10:00:00Z",
};

const role: ModelRole = {
  role_id: "playlist_planner",
  label: "Playlist planner",
  description: "Plans candidate playlists from reviewed library information.",
  required_capability_ids: ["structured-text/v1"],
  configuration_available: true,
  connection_id: null,
  connection_name: null,
  model_id: "",
  enabled: false,
  effective_enabled: false,
  thinking_mode: "provider_default",
  timeout_seconds: 30,
  max_output_tokens: 2000,
  verification_status: null,
  conformance_status: "never",
  conformance_error_code: null,
  last_conformance_at: null,
  updated_at: null,
};

const qualityEvaluation: ModelQualityEvaluation = {
  evaluation_id: "playlist-quality-v1",
  role_id: "playlist_planner",
  label: "Playlist planning quality",
  description: (
    "Runs fixed synthetic D&D playlist scenarios through this model. " +
    "No songs or live library data are sent."
  ),
  status: "never",
  suite_id: "model-dnd-playlist-quality-v5",
  passed_cases: 0,
  total_cases: 0,
  last_job_id: null,
  last_evaluated_at: null,
};

const musicTaggingRole: ModelRole = {
  ...role,
  role_id: "music_tagger",
  label: "Music tagger",
  description: "Suggests controlled D&D tags from indexed metadata.",
  connection_id: connection.id,
  connection_name: connection.name,
  model_id: "tagger-small",
  enabled: true,
  effective_enabled: true,
  verification_status: "verified",
  conformance_status: "passed",
};

const tagCleanupRole: ModelRole = {
  ...musicTaggingRole,
  role_id: "tag_cleanup",
  label: "Mood-tag cleanup",
  description: "Maps existing song tags onto the controlled vocabulary.",
  model_id: "cleanup-fast",
};

const musicTaggingEvaluation: ModelQualityEvaluation = {
  evaluation_id: "music-tagging-quality-v1",
  role_id: "music_tagger",
  label: "Mood tagging quality",
  description: "Runs fixed synthetic metadata cases through this model.",
  status: "never",
  suite_id: "controlled-vocabulary-tagging-baseline-v20",
  passed_cases: 0,
  total_cases: 0,
  last_job_id: null,
  last_evaluated_at: null,
};

function qualityJob(overrides: Partial<BackgroundJob> = {}): BackgroundJob {
  return {
    id: "quality-job-1",
    kind: "assistant.model-evaluation.playlist-quality-v1",
    status: "running",
    parameters: {
      role_id: "playlist_planner",
      evaluation_id: "playlist-quality-v1",
    },
    result: null,
    error: null,
    progress_current: 2,
    progress_total: 5,
    progress_phase: "Evaluating playlist model",
    progress_message: "Completed 2 of 5 synthetic scenarios",
    attempts: 1,
    retry_of_id: null,
    created_at: "2026-08-19T12:00:00Z",
    updated_at: "2026-08-19T12:01:00Z",
    started_at: "2026-08-19T12:00:01Z",
    finished_at: null,
    ...overrides,
  };
}

beforeEach(() => {
  vi.clearAllMocks();
  vi.mocked(assistantProvidersApi.getStatus).mockResolvedValue(frameworkStatus);
  vi.mocked(assistantProvidersApi.listConnections).mockResolvedValue([]);
  vi.mocked(assistantProvidersApi.listRoles).mockResolvedValue([role]);
  vi.mocked(assistantProvidersApi.listRoleEvaluations).mockResolvedValue([]);
  vi.mocked(jobsApi.list).mockResolvedValue([]);
  vi.mocked(confirmDialog).mockResolvedValue(true);
  vi.mocked(inputDialog).mockResolvedValue("test-password");
});

describe("AssistantAiSetupView", () => {
  it("groups tagging and cleanup around their shared vocabulary", async () => {
    vi.mocked(assistantProvidersApi.listConnections).mockResolvedValue([connection]);
    vi.mocked(assistantProvidersApi.listRoles).mockResolvedValue([
      musicTaggingRole,
      tagCleanupRole,
    ]);

    render(<AssistantAiSetupView />);

    const heading = await screen.findByRole("heading", {
      name: "Tag intelligence",
    });
    const family = heading.closest("section");
    expect(family).not.toBeNull();
    expect(
      within(family as HTMLElement).getByRole("heading", {
        name: "Music tagger",
      }),
    ).toBeInTheDocument();
    expect(
      within(family as HTMLElement).getByRole("heading", {
        name: "Mood-tag cleanup",
      }),
    ).toBeInTheDocument();
    expect(
      within(family as HTMLElement).getByText("Canonical tag IDs"),
    ).toBeInTheDocument();
    expect(
      within(family as HTMLElement).getByText("Canonical ID or no match"),
    ).toBeInTheDocument();
  });

  it("explains per-task keys and shows which tasks reuse a connection", async () => {
    vi.mocked(assistantProvidersApi.listConnections).mockResolvedValue([connection]);
    vi.mocked(assistantProvidersApi.listRoles).mockResolvedValue([
      {
        ...role,
        connection_id: connection.id,
        connection_name: connection.name,
        model_id: "planner-large",
      },
      musicTaggingRole,
    ]);

    render(<AssistantAiSetupView />);

    expect(
      await screen.findByRole("heading", { name: "Models and providers" }),
    ).toBeInTheDocument();
    expect(
      screen.queryByText(/Each connection stores one provider key/i),
    ).not.toBeInTheDocument();
    await userEvent.click(
      screen.getByRole("heading", { name: "Hosted models" }).closest("summary")!,
    );
    expect(screen.getByText("Playlist planner · Music tagger")).toBeInTheDocument();
    expect(screen.getByText("Structured text")).toBeInTheDocument();
    expect(
      screen.queryByText(/other tasks may reuse this key/i),
    ).not.toBeInTheDocument();
    expect(
      screen.getByText("Add provider connection").closest("details"),
    ).not.toHaveAttribute("open");
  });

  it("keeps future capability-bound tasks visibly planned and locked", async () => {
    const plannedAudioRole: ModelRole = {
      ...role,
      role_id: "audio_analyzer",
      label: "Specialized audio analysis",
      description: "Reserved for a future audio-capable adapter.",
      required_capability_ids: ["audio-input/v1"],
      configuration_available: false,
    };
    vi.mocked(assistantProvidersApi.listConnections).mockResolvedValue([connection]);
    vi.mocked(assistantProvidersApi.listRoles).mockResolvedValue([
      role,
      plannedAudioRole,
    ]);
    render(<AssistantAiSetupView />);

    const heading = await screen.findByRole("heading", {
      name: "Specialized audio analysis",
    });
    const card = heading.closest("article");
    expect(card).not.toBeNull();
    expect(within(card as HTMLElement).getByText("Planned")).toBeInTheDocument();
    expect(
      within(card as HTMLElement).getByText("Not configurable yet"),
    ).toBeInTheDocument();
    expect(
      within(card as HTMLElement).getByText(/This task will require: Audio input/),
    ).toBeInTheDocument();
    expect(
      within(card as HTMLElement).queryByRole("button", { name: "Save task" }),
    ).not.toBeInTheDocument();
  });

  it("does not allow testing when verification lacks the required capability", async () => {
    const capabilityMissingConnection: ProviderConnection = {
      ...connection,
      verified_capability_ids: [],
    };
    const configuredRole: ModelRole = {
      ...role,
      connection_id: connection.id,
      connection_name: connection.name,
      model_id: "planner-large",
      verification_status: "verified",
    };
    vi.mocked(assistantProvidersApi.listConnections).mockResolvedValue([
      capabilityMissingConnection,
    ]);
    vi.mocked(assistantProvidersApi.listRoles).mockResolvedValue([configuredRole]);
    render(<AssistantAiSetupView />);

    const connectionCard = (await screen.findByRole("heading", {
      name: "Hosted models",
    })).closest("details.assistant-provider-card");
    expect(connectionCard).not.toBeNull();
    await userEvent.click(
      within(connectionCard as HTMLElement).getByRole("heading", { name: "Hosted models" })
        .closest("summary")!,
    );
    expect(
      within(connectionCard as HTMLElement).getByText("None confirmed"),
    ).toBeInTheDocument();
    expect(
      screen.getByRole("button", { name: "Test model and allow" }),
    ).toBeDisabled();
    expect(
      screen.getByText(/Verification did not confirm Structured text/),
    ).toBeInTheDocument();
  });

  it("shows only the models verified for the selected connection", async () => {
    const user = userEvent.setup();
    const localConnection: ProviderConnection = {
      ...connection,
      id: "connection-2",
      name: "Local models",
      base_url: "http://host.docker.internal:11434/v1",
      allow_private_network: true,
      verified_models: ["local-instruct", "local-reasoning"],
    };
    vi.mocked(assistantProvidersApi.listConnections).mockResolvedValue([
      connection,
      localConnection,
    ]);
    render(<AssistantAiSetupView />);

    const connectionPicker = await screen.findByLabelText("Connection");
    await user.selectOptions(connectionPicker, connection.id);
    await user.click(screen.getByLabelText("Model"));
    expect(
      screen.getByRole("option", { name: "planner-large" }),
    ).toBeInTheDocument();
    expect(
      screen.queryByRole("option", { name: "local-instruct" }),
    ).not.toBeInTheDocument();

    await user.keyboard("{Escape}");
    await user.selectOptions(connectionPicker, localConnection.id);
    await user.click(screen.getByLabelText("Model"));
    expect(
      screen.getByRole("option", { name: "local-instruct" }),
    ).toBeInTheDocument();
    expect(
      screen.queryByRole("option", { name: "planner-large" }),
    ).not.toBeInTheDocument();
  });

  it("saves a provider connection and clears the API key from the form", async () => {
    const user = userEvent.setup();
    vi.mocked(assistantProvidersApi.createConnection).mockResolvedValue({
      ...connection,
      verification_status: "never",
      verified_models: [],
      verified_capability_ids: [],
      last_verified_at: null,
    });
    render(<AssistantAiSetupView />);

    await screen.findByRole("heading", { name: "Models and providers" });
    await user.type(screen.getByLabelText("Connection name"), "Hosted models");
    await user.type(
      screen.getByLabelText("Provider address"),
      "https://models.example/v1",
    );
    await user.type(screen.getByLabelText("API key"), TEST_PROVIDER_API_KEY);
    await user.click(screen.getByRole("button", { name: "Save connection" }));

    await waitFor(() =>
      expect(assistantProvidersApi.createConnection).toHaveBeenCalledWith({
        name: "Hosted models",
        adapter_id: "openai-compatible/v1",
        base_url: "https://models.example/v1",
        api_key: TEST_PROVIDER_API_KEY,
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
        verified_capability_ids: [],
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

  it("warns that reverifying an assigned connection resets its model gates", async () => {
    const user = userEvent.setup();
    const assignedRole: ModelRole = {
      ...role,
      connection_id: connection.id,
      connection_name: connection.name,
      model_id: "planner-large",
      enabled: true,
      effective_enabled: true,
      verification_status: "verified",
      conformance_status: "passed",
    };
    vi.mocked(assistantProvidersApi.listConnections).mockResolvedValue([connection]);
    vi.mocked(assistantProvidersApi.listRoles).mockResolvedValue([assignedRole]);
    vi.mocked(assistantProvidersApi.verifyConnection).mockResolvedValue({
      connection,
      verified: true,
      error_code: null,
      models: connection.verified_models,
    });
    render(<AssistantAiSetupView />);

    await user.click(
      (await screen.findByRole("heading", { name: "Hosted models" })).closest("summary")!,
    );
    await user.click(screen.getByRole("button", { name: "Verify again" }));

    expect(confirmDialog).toHaveBeenCalledWith({
      title: "Verify connection again?",
      body:
        "Verifying Hosted models again will clear the model tests and quality " +
        "results for Playlist planner. Wait for or cancel any running model work first.",
      confirmLabel: "Verify and reset tests",
      tone: "primary",
    });
    await waitFor(() =>
      expect(assistantProvidersApi.verifyConnection).toHaveBeenCalledWith(
        "connection-1",
      ),
    );
  });

  it("shows, deletes, and gates use of a saved API key", async () => {
    const user = userEvent.setup();
    const connectionWithoutKey: ProviderConnection = {
      ...connection,
      credential_saved: false,
      key_hint: null,
      verification_status: "never",
      verified_models: [],
      verified_capability_ids: [],
      last_verified_at: null,
    };
    vi.mocked(assistantProvidersApi.listConnections)
      .mockResolvedValueOnce([connection])
      .mockResolvedValue([connectionWithoutKey]);
    vi.mocked(
      assistantProvidersApi.deleteConnectionCredential,
    ).mockResolvedValue(connectionWithoutKey);
    render(<AssistantAiSetupView />);

    expect(await screen.findByText("••••1234")).toBeInTheDocument();
    const connectionCard = screen
      .getByRole("heading", { name: "Hosted models" })
      .closest("details.assistant-provider-card");
    expect(connectionCard).not.toBeNull();
    await user.click(
      within(connectionCard as HTMLElement).getByRole("heading", { name: "Hosted models" })
        .closest("summary")!,
    );
    await user.click(
      within(connectionCard as HTMLElement).getByText("Connection settings"),
    );
    expect(
      within(connectionCard as HTMLElement).getByText(
        /cannot be replaced in place/i,
      ),
    ).toBeInTheDocument();
    expect(
      within(connectionCard as HTMLElement).queryByLabelText("Replace API key"),
    ).not.toBeInTheDocument();
    await user.click(screen.getByRole("button", { name: "Delete API key" }));

    expect(confirmDialog).toHaveBeenCalledWith({
      title: "Delete saved API key?",
      body: expect.stringContaining("cannot run until you save and verify a new key"),
      confirmLabel: "Delete API key",
      tone: "danger",
    });
    await waitFor(() =>
      expect(
        assistantProvidersApi.deleteConnectionCredential,
      ).toHaveBeenCalledWith("connection-1"),
    );
    expect(await screen.findByText("Missing")).toBeInTheDocument();
    expect(
      screen.getByRole("button", { name: "Verify connection" }),
    ).toBeDisabled();
    expect(
      screen.getByRole("heading", { name: "Hosted models" }),
    ).toBeInTheDocument();
    expect(toast.success).toHaveBeenCalledWith(
      "API key deleted",
      "The connection was kept. Save and verify a new key before using it again.",
    );
  });

  it("saves, tests, and automatically allows one model task", async () => {
    const user = userEvent.setup();
    vi.mocked(assistantProvidersApi.listConnections).mockResolvedValue([connection]);
    const configuredRole: ModelRole = {
      ...role,
      connection_id: connection.id,
      connection_name: connection.name,
      model_id: "planner-large",
      enabled: false,
      effective_enabled: false,
      thinking_mode: "disabled",
      verification_status: "verified",
    };
    const testedRole: ModelRole = {
      ...configuredRole,
      conformance_status: "passed",
      last_conformance_at: "2026-08-19T11:00:00Z",
    };
    const enabledRole: ModelRole = {
      ...testedRole,
      enabled: true,
      effective_enabled: true,
    };
    vi.mocked(assistantProvidersApi.updateRole)
      .mockResolvedValueOnce(configuredRole)
      .mockResolvedValueOnce(enabledRole);
    vi.mocked(assistantProvidersApi.testRole).mockResolvedValue({
      role: testedRole,
      passed: true,
      error_code: null,
      contract_version: "assistant-provider-conformance/v3",
      provider_model_id: "planner-large-2026",
      finish_reason: "stop",
      input_tokens: 42,
      output_tokens: 18,
      duration_ms: 812,
    });
    render(<AssistantAiSetupView />);

    await user.selectOptions(await screen.findByLabelText("Connection"), connection.id);
    const allowCheckbox = screen.getByLabelText(
      "Allow this model for this task",
    );
    expect(allowCheckbox.closest(".assistant-role-heading")).not.toBeNull();
    await user.click(screen.getByText("Request settings"));
    expect(screen.getByRole("group", { name: "Request settings" })).toBeVisible();
    expect(screen.getByLabelText("Provider default")).toBeVisible();
    expect(screen.getByText("Off recommended")).toBeVisible();
    const modelPicker = screen.getByLabelText("Model");
    await user.click(modelPicker);
    expect(screen.getByText("2 available models")).toBeInTheDocument();
    await user.type(modelPicker, "planner");
    expect(screen.getByText("1 of 2 models")).toBeInTheDocument();
    expect(
      screen.queryByRole("option", { name: "tagger-small" }),
    ).not.toBeInTheDocument();
    await user.click(screen.getByRole("option", { name: "planner-large" }));
    expect(modelPicker).toHaveValue("planner-large");
    await user.click(screen.getByLabelText("Off"));
    await user.click(screen.getByRole("button", { name: "Save task" }));

    await waitFor(() =>
      expect(assistantProvidersApi.updateRole).toHaveBeenCalledWith(
        "playlist_planner",
        {
          connection_id: "connection-1",
          model_id: "planner-large",
          enabled: false,
          thinking_mode: "disabled",
          timeout_seconds: 30,
          max_output_tokens: 2000,
        },
      ),
    );
    await user.click(
      await screen.findByRole("button", { name: "Test model and allow" }),
    );
    await waitFor(() =>
      expect(assistantProvidersApi.testRole).toHaveBeenCalledWith(
        "playlist_planner",
      ),
    );
    await waitFor(() =>
      expect(assistantProvidersApi.updateRole).toHaveBeenLastCalledWith(
        "playlist_planner",
        {
          connection_id: "connection-1",
          model_id: "planner-large",
          enabled: true,
          thinking_mode: "disabled",
          timeout_seconds: 30,
          max_output_tokens: 2000,
        },
      ),
    );
    expect(
      screen.getByLabelText("Allow this model for this task"),
    ).toBeChecked();
    expect(toast.success).toHaveBeenCalledWith(
      "Model tested and allowed",
      "Playlist planner",
    );
  });

  it("keeps a passed model disabled when the automatic allow save fails", async () => {
    const user = userEvent.setup();
    const configuredRole: ModelRole = {
      ...role,
      connection_id: connection.id,
      connection_name: connection.name,
      model_id: "planner-large",
      verification_status: "verified",
    };
    const testedRole: ModelRole = {
      ...configuredRole,
      conformance_status: "passed",
      last_conformance_at: "2026-08-19T11:00:00Z",
    };
    vi.mocked(assistantProvidersApi.listConnections).mockResolvedValue([connection]);
    vi.mocked(assistantProvidersApi.listRoles).mockResolvedValue([configuredRole]);
    vi.mocked(assistantProvidersApi.testRole).mockResolvedValue({
      role: testedRole,
      passed: true,
      error_code: null,
      contract_version: "assistant-provider-conformance/v3",
      provider_model_id: "planner-large-2026",
      finish_reason: "stop",
      input_tokens: 42,
      output_tokens: 18,
      duration_ms: 812,
    });
    vi.mocked(assistantProvidersApi.updateRole).mockRejectedValue(
      new Error("Role changed while the model test was running."),
    );
    render(<AssistantAiSetupView />);

    await user.click(
      await screen.findByRole("button", { name: "Test model and allow" }),
    );

    await waitFor(() =>
      expect(toast.error).toHaveBeenCalledWith(
        "Model passed but could not be allowed",
        "Role changed while the model test was running.",
      ),
    );
    expect(
      screen.getByLabelText("Allow this model for this task"),
    ).not.toBeChecked();
    expect(
      screen.getByText(/Model test passed\. Select “Allow for task”/),
    ).toBeVisible();
  });

  it("keeps a failed model test visible and the task disabled", async () => {
    const user = userEvent.setup();
    const configuredRole: ModelRole = {
      ...role,
      connection_id: connection.id,
      connection_name: connection.name,
      model_id: "planner-large",
      verification_status: "verified",
    };
    const failedRole: ModelRole = {
      ...configuredRole,
      conformance_status: "failed",
      conformance_error_code: "invalid_structured_output",
      last_conformance_at: "2026-08-19T11:00:00Z",
    };
    vi.mocked(assistantProvidersApi.listConnections).mockResolvedValue([connection]);
    vi.mocked(assistantProvidersApi.listRoles).mockResolvedValue([configuredRole]);
    vi.mocked(assistantProvidersApi.testRole).mockResolvedValue({
      role: failedRole,
      passed: false,
      error_code: "invalid_structured_output",
      contract_version: "assistant-provider-conformance/v3",
      provider_model_id: "planner-large-2026",
      finish_reason: "stop",
      input_tokens: 39,
      output_tokens: 11,
      duration_ms: 947,
    });
    render(<AssistantAiSetupView />);

    await user.click(
      await screen.findByRole("button", { name: "Test model and allow" }),
    );

    await waitFor(() =>
      expect(toast.error).toHaveBeenCalledWith(
        "Model test failed",
        "The model did not return the required machine-readable JSON object.",
      ),
    );
    expect(
      screen.getAllByText(
        "The model did not return the required machine-readable JSON object.",
      ),
    ).toHaveLength(2);
    expect(
      screen.getByLabelText("Allow this model for this task"),
    ).toBeDisabled();
    expect(
      screen.queryByText(/no songs or live library data/i),
    ).not.toBeInTheDocument();
    expect(
      screen.getByLabelText("Selected model task diagnostics JSON"),
    ).toHaveTextContent(/"provider_model_id":\s*"planner-large-2026"/);
    await user.click(
      screen.getByRole("button", { name: "Copy details" }),
    );
    const copiedDetails = await navigator.clipboard.readText();
    expect(copiedDetails).toContain('"duration_ms": 947');
    expect(copiedDetails).toContain('"thinking_mode": "provider_default"');
    expect(copiedDetails).toContain(
      '"adapter_label": "OpenAI-compatible API"',
    );
    expect(copiedDetails).toContain(
      '"base_url": "https://models.example/v1"',
    );
    expect(toast.success).toHaveBeenCalledWith("Test diagnostics copied");
  });

  it("keeps local tools available when encrypted storage is not configured", async () => {
    vi.mocked(assistantProvidersApi.getStatus).mockResolvedValue({
      ...frameworkStatus,
      credential_storage_ready: false,
      credential_storage_error: "master_key_not_configured",
      credential_storage_source: null,
      credential_storage_key_id: null,
      credential_storage_key_file_path:
        "/run/music-secrets/assistant-credential.key",
      credential_storage_host_directory_hint: "/opt/stacks/music/secrets",
      credential_storage_can_initialize: false,
      credential_storage_initialization_error:
        "master_key_directory_unavailable",
    });
    render(<AssistantAiSetupView />);

    expect(
      await screen.findByText("Secure key storage needs server setup"),
    ).toBeInTheDocument();
    expect(
      screen.getByText(/Local analysis and playlist building remain available/),
    ).toBeInTheDocument();
    expect(screen.queryByLabelText("API key")).not.toBeInTheDocument();
    await userEvent.click(
      screen.getByRole("button", { name: "Show maintenance guide" }),
    );
    expect(
      screen.getByText(
        "sudo install -d -m 0700 -o 1000 -g 1000 /opt/stacks/music/secrets",
      ),
    ).toBeInTheDocument();
  });

  it("initializes fixed key-file storage from the authenticated UI", async () => {
    const missingStorage: ProviderFrameworkStatus = {
      ...frameworkStatus,
      credential_storage_ready: false,
      credential_storage_error: "master_key_not_configured",
      credential_storage_source: null,
      credential_storage_key_id: null,
      credential_storage_key_file_path:
        "/run/music-secrets/assistant-credential.key",
      credential_storage_can_initialize: true,
      credential_storage_initialization_error: null,
    };
    const initializedStorage: ProviderFrameworkStatus = {
      ...frameworkStatus,
      credential_storage_source: "file",
      credential_storage_key_file_path:
        "/run/music-secrets/assistant-credential.key",
    };
    vi.mocked(assistantProvidersApi.getStatus).mockResolvedValue(missingStorage);
    vi.mocked(
      assistantProvidersApi.initializeCredentialStorage,
    ).mockResolvedValue(initializedStorage);

    render(<AssistantAiSetupView />);

    await userEvent.click(
      await screen.findByRole("button", { name: "Initialize secure storage" }),
    );

    await waitFor(() =>
      expect(
        assistantProvidersApi.initializeCredentialStorage,
      ).toHaveBeenCalledOnce(),
    );
    expect(
      await screen.findByText("Encrypted key storage is ready"),
    ).toBeInTheDocument();
    expect(screen.getByLabelText("API key")).toBeInTheDocument();
    expect(toast.success).toHaveBeenCalledWith(
      "Encrypted storage initialized",
      "You can now save model-provider and catalog API keys.",
    );
  });

  it("shows UI reset and safe rotation guidance for file storage", async () => {
    vi.mocked(assistantProvidersApi.getStatus).mockResolvedValue({
      ...frameworkStatus,
      credential_storage_source: "file",
      credential_storage_key_file_path:
        "/run/music-secrets/assistant-credential.key",
    });
    render(<AssistantAiSetupView />);

    expect(
      await screen.findByRole("button", { name: "Reset encrypted key storage" }),
    ).toBeInTheDocument();

    await userEvent.click(
      screen.getByRole("button", { name: "Show maintenance guide" }),
    );

    expect(screen.getByText("Server maintenance")).toBeInTheDocument();
    expect(
      screen.getByText(/erases every saved API key before it removes/i),
    ).toBeInTheDocument();
    expect(
      screen.getByText(/assistant-credentials rotate/),
    ).toBeInTheDocument();
  });

  it("password-confirms and completely resets file-backed AI storage", async () => {
    const fileStorage: ProviderFrameworkStatus = {
      ...frameworkStatus,
      credential_storage_source: "file",
      credential_storage_key_file_path:
        "/run/music-secrets/assistant-credential.key",
    };
    const resetStorage: ProviderFrameworkStatus = {
      ...fileStorage,
      credential_storage_ready: false,
      credential_storage_error: "master_key_not_configured",
      credential_storage_source: null,
      credential_storage_key_id: null,
      credential_storage_can_initialize: true,
      credential_storage_initialization_error: null,
    };
    vi.mocked(assistantProvidersApi.getStatus)
      .mockResolvedValueOnce(fileStorage)
      .mockResolvedValue(resetStorage);
    vi.mocked(assistantProvidersApi.listConnections)
      .mockResolvedValueOnce([connection])
      .mockResolvedValue([
        {
          ...connection,
          credential_saved: false,
          key_hint: null,
          verification_status: "never",
          verified_models: [],
          verified_capability_ids: [],
          last_verified_at: null,
        },
      ]);
    vi.mocked(assistantProvidersApi.resetCredentialStorage).mockResolvedValue({
      deleted_credentials: 1,
      master_key_removed: true,
      master_key_removal_error: null,
      status: resetStorage,
    });
    render(<AssistantAiSetupView />);

    await userEvent.click(
      await screen.findByRole("button", { name: "Reset encrypted key storage" }),
    );

    expect(confirmDialog).toHaveBeenCalledWith({
      title: "Reset all encrypted key storage?",
      body: expect.stringContaining("permanently deleted"),
      confirmLabel: "Continue to password",
      tone: "danger",
    });
    expect(inputDialog).toHaveBeenCalledWith({
      title: "Confirm encrypted storage reset",
      body: expect.stringContaining("not stored"),
      label: "Current password",
      type: "password",
      confirmLabel: "Delete all saved API keys",
      trim: false,
    });
    await waitFor(() =>
      expect(assistantProvidersApi.resetCredentialStorage).toHaveBeenCalledWith(
        "test-password",
      ),
    );
    expect(toast.success).toHaveBeenCalledWith(
      "Encrypted key storage reset",
      "1 saved API key was deleted. Connection and task drafts were kept.",
    );
    expect(
      await screen.findByRole("button", { name: "Initialize secure storage" }),
    ).toBeInTheDocument();
  });

  it("restores playlist quality progress and can cancel after a refresh", async () => {
    const enabledRole: ModelRole = {
      ...role,
      connection_id: connection.id,
      connection_name: connection.name,
      model_id: "planner-large",
      enabled: true,
      effective_enabled: true,
      verification_status: "verified",
      conformance_status: "passed",
    };
    const running = qualityJob();
    vi.mocked(assistantProvidersApi.listConnections).mockResolvedValue([connection]);
    vi.mocked(assistantProvidersApi.listRoles).mockResolvedValue([enabledRole]);
    vi.mocked(assistantProvidersApi.listRoleEvaluations).mockResolvedValue([
      qualityEvaluation,
    ]);
    vi.mocked(jobsApi.list).mockResolvedValue([running]);
    vi.mocked(jobsApi.cancel).mockResolvedValue({
      ...running,
      status: "cancel_requested",
      progress_phase: "Cancelling",
    });
    const user = userEvent.setup();
    render(<AssistantAiSetupView />);

    expect(
      await screen.findByText("Completed 2 of 5 synthetic scenarios"),
    ).toBeInTheDocument();
    expect(
      screen.getByRole("progressbar", {
        name: "Playlist model quality progress",
      }),
    ).toHaveValue(2);
    const plannerCard = screen
      .getByRole("heading", { name: "Playlist planner" })
      .closest("article");
    expect(plannerCard).not.toBeNull();
    expect(
      within(plannerCard as HTMLElement).getByText("2 / 5 scenarios"),
    ).toBeInTheDocument();

    await user.click(
      screen.getByRole("button", { name: "Cancel quality check" }),
    );
    expect(jobsApi.cancel).toHaveBeenCalledWith("quality-job-1");
  });

  it("labels mood-tagging progress as scored attempts rather than scenarios", async () => {
    const running = qualityJob({
      kind: "assistant.model-evaluation.music-tagging-quality-v1",
      parameters: {
        role_id: "music_tagger",
        evaluation_id: "music-tagging-quality-v1",
      },
      progress_current: 4,
      progress_total: 50,
      progress_phase: "Evaluating music tagger",
      progress_message:
        "Completed 4 of 50 scored attempts across 43 scored scenarios",
    });
    vi.mocked(assistantProvidersApi.listConnections).mockResolvedValue([connection]);
    vi.mocked(assistantProvidersApi.listRoles).mockResolvedValue([
      musicTaggingRole,
    ]);
    vi.mocked(assistantProvidersApi.listRoleEvaluations).mockResolvedValue([
      musicTaggingEvaluation,
    ]);
    vi.mocked(jobsApi.list).mockResolvedValue([running]);
    render(<AssistantAiSetupView />);

    expect(
      await screen.findByText(
        "Completed 4 of 50 scored attempts across 43 scored scenarios",
      ),
    ).toBeInTheDocument();
    const taggerCard = screen
      .getByRole("heading", { name: "Music tagger" })
      .closest("article");
    expect(taggerCard).not.toBeNull();
    expect(
      within(taggerCard as HTMLElement).getByText("4 / 50 scored attempts"),
    ).toBeInTheDocument();
  });

  it("keeps an obsolete interrupted attempt in the troubleshooting log", async () => {
    const user = userEvent.setup();
    const interrupted = qualityJob({
      status: "failed",
      error: "ProviderServiceError: Test this model configuration before using the role.",
      progress_phase: "Interrupted",
      progress_message: "",
      finished_at: "2026-08-19T12:05:00Z",
    });
    vi.mocked(assistantProvidersApi.listConnections).mockResolvedValue([connection]);
    vi.mocked(assistantProvidersApi.listRoles).mockResolvedValue([
      {
        ...role,
        connection_id: connection.id,
        connection_name: connection.name,
        model_id: "planner-large",
        enabled: true,
        effective_enabled: false,
        verification_status: "verified",
        conformance_status: "never",
        last_conformance_at: null,
      },
    ]);
    vi.mocked(assistantProvidersApi.listRoleEvaluations).mockResolvedValue([
      qualityEvaluation,
    ]);
    vi.mocked(jobsApi.list).mockResolvedValue([interrupted]);
    render(<AssistantAiSetupView />);

    expect(
      await screen.findByText(/quality suite has not run/i),
    ).toBeInTheDocument();
    expect(screen.getAllByText("Not run")).not.toHaveLength(0);
    expect(
      screen.queryByRole("button", { name: "Run quality check" }),
    ).toBeNull();
    expect(
      screen.getByRole("button", { name: "Test model and allow" }),
    ).toBeEnabled();
    await user.click(screen.getByText("Test console"));
    expect(
      within(screen.getByRole("log")).getByText(
        /Test this model configuration before using the role\./,
      ),
    ).toBeVisible();
  });

  it("shows an interruption from the currently tested model configuration", async () => {
    const interrupted = qualityJob({
      status: "failed",
      error: "Provider request timed out.",
      progress_phase: "Interrupted",
      progress_message: "",
      finished_at: "2026-08-19T12:05:00Z",
    });
    vi.mocked(assistantProvidersApi.listConnections).mockResolvedValue([connection]);
    vi.mocked(assistantProvidersApi.listRoles).mockResolvedValue([
      {
        ...role,
        connection_id: connection.id,
        connection_name: connection.name,
        model_id: "planner-large",
        enabled: true,
        effective_enabled: true,
        verification_status: "verified",
        conformance_status: "passed",
        last_conformance_at: "2026-08-19T11:00:00Z",
      },
    ]);
    vi.mocked(assistantProvidersApi.listRoleEvaluations).mockResolvedValue([
      qualityEvaluation,
    ]);
    vi.mocked(jobsApi.list).mockResolvedValue([interrupted]);
    render(<AssistantAiSetupView />);

    expect(
      await screen.findByText("Provider request timed out."),
    ).toBeInTheDocument();
    expect(screen.getAllByText("Interrupted")).not.toHaveLength(0);
    expect(
      screen.getByRole("button", { name: "Run quality again" }),
    ).toBeEnabled();
  });

  it("starts the synthetic quality check only after explicit confirmation", async () => {
    const enabledRole: ModelRole = {
      ...role,
      connection_id: connection.id,
      connection_name: connection.name,
      model_id: "planner-large",
      enabled: true,
      effective_enabled: true,
      verification_status: "verified",
      conformance_status: "passed",
    };
    const queued = qualityJob({
      status: "queued",
      progress_current: 0,
      progress_total: null,
      progress_phase: "Queued",
      progress_message: "",
      started_at: null,
    });
    vi.mocked(assistantProvidersApi.listConnections).mockResolvedValue([connection]);
    vi.mocked(assistantProvidersApi.listRoles).mockResolvedValue([enabledRole]);
    vi.mocked(assistantProvidersApi.listRoleEvaluations).mockResolvedValue([
      qualityEvaluation,
    ]);
    vi.mocked(assistantProvidersApi.startRoleEvaluation).mockResolvedValue(queued);
    const user = userEvent.setup();
    render(<AssistantAiSetupView />);

    await user.click(
      await screen.findByRole("button", { name: "Run quality check" }),
    );

    expect(confirmDialog).toHaveBeenCalledWith(
      expect.objectContaining({
        title: "Run playlist model quality check?",
        confirmLabel: "Run quality check",
      }),
    );
    await waitFor(() =>
      expect(assistantProvidersApi.startRoleEvaluation).toHaveBeenCalledWith(
        "playlist_planner",
        "playlist-quality-v1",
      ),
    );
    expect(toast.success).toHaveBeenCalledWith(
      "Model quality check queued",
      "You can leave this page; progress is stored on the server.",
    );
  });

  it("describes EQ quality checks as EQ work, not playlist work", async () => {
    const eqRole: ModelRole = {
      ...musicTaggingRole,
      role_id: "eq_assistant",
      label: "EQ assistance",
      model_id: "planner-large",
    };
    const eqEvaluation: ModelQualityEvaluation = {
      ...qualityEvaluation,
      evaluation_id: "eq-quality-v1",
      role_id: "eq_assistant",
      label: "EQ drafting quality",
      suite_id: "eq-quality-v1",
    };
    vi.mocked(assistantProvidersApi.listConnections).mockResolvedValue([connection]);
    vi.mocked(assistantProvidersApi.listRoles).mockResolvedValue([eqRole]);
    vi.mocked(assistantProvidersApi.listRoleEvaluations).mockImplementation(
      async (roleId) => (roleId === "eq_assistant" ? [eqEvaluation] : []),
    );
    vi.mocked(confirmDialog).mockResolvedValueOnce(false);
    const user = userEvent.setup();
    render(<AssistantAiSetupView />);

    await user.click(
      await screen.findByRole("button", { name: "Run quality check" }),
    );

    expect(confirmDialog).toHaveBeenCalledWith(
      expect.objectContaining({
        title: "Run EQ assistant model quality check?",
        body: expect.stringContaining("fixed synthetic EQ drafting scenarios"),
      }),
    );
  });

  it("keeps music tagging quality separate from playlist planning", async () => {
    const queued = qualityJob({
      id: "tagging-quality-job",
      kind: "assistant.model-evaluation.music-tagging-quality-v1",
      status: "queued",
      parameters: {
        role_id: "music_tagger",
        evaluation_id: "music-tagging-quality-v1",
      },
      progress_current: 0,
      progress_total: null,
      progress_phase: "Queued",
      progress_message: "",
      started_at: null,
    });
    vi.mocked(assistantProvidersApi.listConnections).mockResolvedValue([connection]);
    vi.mocked(assistantProvidersApi.listRoles).mockResolvedValue([
      role,
      musicTaggingRole,
    ]);
    vi.mocked(assistantProvidersApi.listRoleEvaluations).mockImplementation(
      async (roleId) =>
        roleId === "music_tagger" ? [musicTaggingEvaluation] : [],
    );
    vi.mocked(assistantProvidersApi.startRoleEvaluation).mockResolvedValue(queued);
    const user = userEvent.setup();
    render(<AssistantAiSetupView />);

    const heading = await screen.findByRole("heading", {
      name: "Music tagger",
    });
    const card = heading.closest("article");
    expect(card).not.toBeNull();
    await user.click(within(card as HTMLElement).getByText("Request settings"));
    expect(within(card as HTMLElement).getByText("Off recommended")).toBeVisible();
    await user.click(
      within(card as HTMLElement).getByRole("button", {
        name: "Run quality check",
      }),
    );

    expect(confirmDialog).toHaveBeenCalledWith(
      expect.objectContaining({
        title: "Run mood tagging model quality check?",
      }),
    );
    await waitFor(() =>
      expect(assistantProvidersApi.startRoleEvaluation).toHaveBeenCalledWith(
        "music_tagger",
        "music-tagging-quality-v1",
      ),
    );
  });

  it("recommends provider-default thinking for Gemini connections", async () => {
    const geminiConnection: ProviderConnection = {
      ...connection,
      adapter_id: "google-gemini-openai/v1",
      name: "Gemini",
      base_url: "https://generativelanguage.googleapis.com/v1beta/openai",
    };
    const configuredRole: ModelRole = {
      ...role,
      connection_id: geminiConnection.id,
      connection_name: geminiConnection.name,
      model_id: "gemini-2.5-flash",
      verification_status: "verified",
    };
    vi.mocked(assistantProvidersApi.listConnections).mockResolvedValue([
      geminiConnection,
    ]);
    vi.mocked(assistantProvidersApi.listRoles).mockResolvedValue([configuredRole]);
    render(<AssistantAiSetupView />);

    await userEvent.click(await screen.findByText("Request settings"));

    expect(screen.getByText("Provider default first")).toBeVisible();
    expect(screen.queryByText("Off recommended")).not.toBeInTheDocument();
  });

  it("rechecks only failed mood scenarios from the test console", async () => {
    const completed = qualityJob({
      id: "tagging-complete-job",
      kind: "assistant.model-evaluation.music-tagging-quality-v1",
      status: "succeeded",
      parameters: {
        role_id: "music_tagger",
        evaluation_id: "music-tagging-quality-v1",
      },
      result: {
        evaluation: {
          passed: true,
          passed_cases: 39,
          total_cases: 40,
          safety_passed_cases: 7,
          safety_total_cases: 7,
          quality_passed_cases: 39,
          quality_total_cases: 40,
          minimum_quality_pass_rate: 0.9,
          cases: [
            {
              id: "stormy-sea-battle",
              description: "Recognizes a stormy sea battle",
              passed: false,
              blocking: false,
              failures: ["Missing required tags: combat"],
            },
          ],
        },
      },
      finished_at: "2026-08-23T12:05:00Z",
    });
    const queued = qualityJob({
      id: "tagging-retest-job",
      kind: "assistant.model-evaluation.music-tagging-quality-v1",
      status: "queued",
      parameters: {
        role_id: "music_tagger",
        evaluation_id: "music-tagging-quality-v1",
        case_ids: ["stormy-sea-battle"],
        baseline_job_id: completed.id,
      },
    });
    vi.mocked(assistantProvidersApi.listConnections).mockResolvedValue([connection]);
    vi.mocked(assistantProvidersApi.listRoles).mockResolvedValue([
      musicTaggingRole,
    ]);
    vi.mocked(assistantProvidersApi.listRoleEvaluations).mockResolvedValue([
      {
        ...musicTaggingEvaluation,
        status: "passed",
        passed_cases: 39,
        total_cases: 40,
        last_job_id: completed.id,
        last_evaluated_at: completed.finished_at,
      },
    ]);
    vi.mocked(jobsApi.list).mockResolvedValue([completed]);
    vi.mocked(assistantProvidersApi.retestFailedScenarios).mockResolvedValue(
      queued,
    );
    const user = userEvent.setup();
    render(<AssistantAiSetupView />);

    expect(
      await screen.findByText(/Quality gate passed 39 of 40 scenarios/),
    ).toBeInTheDocument();
    expect(
      screen.getByText(
        /score covers 40 distinct scenarios.*7 safety scenarios run twice.*47 model attempts.*contract-recovery requests.*provider usage/i,
      ),
    ).toBeInTheDocument();
    const recheck = screen.getByRole("button", {
      name: "Recheck 1 failed scenario",
    });
    await user.click(recheck);

    expect(confirmDialog).toHaveBeenCalledWith(
      expect.objectContaining({
        title: "Recheck failed mood-tagging scenarios?",
        body: expect.stringMatching(/diagnostic only.*complete suite/i),
      }),
    );
    await waitFor(() =>
      expect(assistantProvidersApi.retestFailedScenarios).toHaveBeenCalledWith(
        "music_tagger",
        "music-tagging-quality-v1",
      ),
    );
  });

  it("groups a suite-wide provider rejection instead of presenting semantic failures", async () => {
    const completed = qualityJob({
      id: "tagging-provider-rejection",
      kind: "assistant.model-evaluation.music-tagging-quality-v1",
      status: "succeeded",
      parameters: {
        role_id: "music_tagger",
        evaluation_id: "music-tagging-quality-v1",
      },
      result: {
        evaluation: {
          passed: false,
          passed_cases: 0,
          total_cases: 2,
          safety_passed_cases: 0,
          safety_total_cases: 1,
          quality_passed_cases: 0,
          quality_total_cases: 2,
          minimum_quality_pass_rate: 0.9,
          cases: [
            {
              id: "tavern-dance",
              description: "Tavern dancing",
              passed: false,
              blocking: true,
              failures: [
                "Tagger error: model_execution_invalid_request",
              ],
            },
            {
              id: "stormy-sea-battle",
              description: "Stormy sea battle",
              passed: false,
              blocking: true,
              failures: [
                "Tagger error: model_execution_invalid_request",
                "Safety repeat: Tagger error: model_execution_invalid_request",
              ],
            },
          ],
        },
        usage: {
          attempted_requests: 1,
          input_tokens: 0,
          output_tokens: 0,
          input_tokens_reported_requests: 0,
          output_tokens_reported_requests: 0,
          provider_model_ids: [],
          provider_model_ids_truncated: false,
        },
      },
      finished_at: "2026-08-23T12:05:00Z",
    });
    vi.mocked(assistantProvidersApi.listConnections).mockResolvedValue([connection]);
    vi.mocked(assistantProvidersApi.listRoles).mockResolvedValue([
      musicTaggingRole,
    ]);
    vi.mocked(assistantProvidersApi.listRoleEvaluations).mockResolvedValue([
      {
        ...musicTaggingEvaluation,
        status: "failed",
        passed_cases: 0,
        total_cases: 2,
        last_job_id: completed.id,
        last_evaluated_at: completed.finished_at,
      },
    ]);
    vi.mocked(jobsApi.list).mockResolvedValue([completed]);
    render(<AssistantAiSetupView />);

    expect(
      await screen.findByText(/suite could not execute model requests/i),
    ).toBeInTheDocument();
    expect(
      screen.getByText(/2 scenario failures share one provider execution cause/i),
    ).toBeInTheDocument();
    expect(screen.queryByText(/Tavern dancing:/)).not.toBeInTheDocument();
    expect(
      screen.queryByRole("button", { name: /Recheck 2 failed scenarios/i }),
    ).not.toBeInTheDocument();
  });

  it("shows a completed failed report without treating the job as broken", async () => {
    const completed = qualityJob({
      status: "succeeded",
      progress_current: 5,
      result: {
        evaluation: {
          cases: [
            {
              id: "tavern-dance",
              description: "Tavern dancing",
              passed: false,
              failures: [
                "recall_at_k below threshold",
                "engine error: model_execution_empty_structured_output",
              ],
            },
          ],
        },
      },
      progress_phase: "Complete",
      progress_message: "Completed 5 of 5 synthetic scenarios",
      finished_at: "2026-08-19T12:05:00Z",
    });
    vi.mocked(assistantProvidersApi.listConnections).mockResolvedValue([connection]);
    vi.mocked(assistantProvidersApi.listRoles).mockResolvedValue([
      {
        ...role,
        connection_id: connection.id,
        model_id: "planner-large",
        enabled: true,
        effective_enabled: true,
        thinking_mode: "enabled",
      },
    ]);
    vi.mocked(assistantProvidersApi.listRoleEvaluations).mockResolvedValue([
      {
        ...qualityEvaluation,
        status: "failed",
        passed_cases: 3,
        total_cases: 5,
        last_job_id: completed.id,
        last_evaluated_at: completed.finished_at,
      },
    ]);
    vi.mocked(jobsApi.list).mockResolvedValue([completed]);
    const user = userEvent.setup();
    render(<AssistantAiSetupView />);

    expect(
      await screen.findByText(/Task quality passed 3 of 5 scenarios/),
    ).toBeInTheDocument();
    expect(
      screen.queryByText("Tavern dancing: recall_at_k below threshold"),
    ).not.toBeInTheDocument();
    expect(
      screen.getByText(/Tavern dancing: recall_at_k below threshold; engine error:/),
    ).toBeInTheDocument();
    expect(
      screen.getByText(/Turn Thinking Off for this task and rerun/),
    ).toBeInTheDocument();
    const taskCard = screen
      .getByRole("heading", { name: "Playlist planner" })
      .closest("article");
    expect(taskCard).not.toBeNull();
    expect(
      within(taskCard as HTMLElement).getByText("3 / 5 scenarios"),
    ).toBeInTheDocument();
    expect(
      within(taskCard as HTMLElement).getByText("Quality check failed"),
    ).toBeInTheDocument();
    await user.click(
      screen.getByRole("button", { name: "Copy details" }),
    );
    await expect(navigator.clipboard.readText()).resolves.toContain(
      '"recall_at_k below threshold"',
    );
  });
});
