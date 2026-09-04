import { render, screen, waitFor } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { MemoryRouter } from "react-router-dom";
import { beforeEach, describe, expect, it, vi } from "vitest";

import type * as ApiModule from "@/core/api";
import type * as ProviderApiModule from "@/core/assistantProvidersApi";
import type { CleanupSource } from "@/core/api";
import type { ProviderFrameworkStatus } from "@/core/assistantProvidersApi";

vi.mock("@/core/api", async (importActual) => {
  const actual = await importActual<typeof ApiModule>();
  return {
    ...actual,
    cleanupApi: {
      ...actual.cleanupApi,
      sources: vi.fn(),
      updateSource: vi.fn(),
      saveSourceCredential: vi.fn(),
      deleteSourceCredential: vi.fn(),
      batches: vi.fn(),
    },
  };
});

vi.mock("@/core/assistantProvidersApi", async (importActual) => {
  const actual = await importActual<typeof ProviderApiModule>();
  return {
    ...actual,
    assistantProvidersApi: {
      ...actual.assistantProvidersApi,
      getStatus: vi.fn(),
      initializeCredentialStorage: vi.fn(),
    },
  };
});

vi.mock("@/components/confirmDialog", () => ({ confirmDialog: vi.fn() }));
vi.mock("@/core/toast", () => ({
  toast: { success: vi.fn(), error: vi.fn(), warn: vi.fn() },
}));

import { confirmDialog } from "@/components/confirmDialog";
import { cleanupApi } from "@/core/api";
import { assistantProvidersApi } from "@/core/assistantProvidersApi";

import {
  LibraryCleanupHistoryView,
  LibraryCleanupSourcesView,
} from "./LibraryCleanupViews";

const musicBrainzSource: CleanupSource = {
  id: "musicbrainz",
  label: "MusicBrainz",
  description: "Checks ambiguous names.",
  enabled: true,
  capabilities: ["artist_name_verification", "album_name_verification"],
  credential_kind: null,
  configured: true,
  available: true,
  credential_saved: false,
  credential_source: null,
  key_hint: null,
  configuration_hint: null,
  unavailable_reason: null,
};

const acoustIdSource: CleanupSource = {
  id: "acoustid",
  label: "AcoustID",
  description: "Uses a Chromaprint fingerprint when text metadata is ambiguous.",
  enabled: true,
  capabilities: ["acoustic_fingerprint_identity"],
  credential_kind: "application API key",
  configured: false,
  available: false,
  credential_saved: false,
  credential_source: null,
  key_hint: null,
  configuration_hint: "Save a client key here or provide ACOUSTID_API_KEY on the server.",
  unavailable_reason: "AcoustID needs a client key before it can run.",
};

const frameworkStatus: ProviderFrameworkStatus = {
  credential_storage_ready: true,
  credential_storage_error: null,
  credential_storage_source: "environment",
  credential_storage_key_id: "0123456789abcdef",
  credential_storage_key_file_path: null,
  credential_storage_host_directory_hint: null,
  credential_storage_can_initialize: false,
  credential_storage_initialization_error: "master_key_already_configured",
  capabilities: [],
  adapters: [],
  roles: [],
};

describe("Library cleanup workspace views", () => {
  beforeEach(() => {
    vi.clearAllMocks();
    vi.mocked(cleanupApi.sources).mockResolvedValue([musicBrainzSource]);
    vi.mocked(cleanupApi.updateSource).mockResolvedValue({
      ...musicBrainzSource,
      enabled: false,
    });
    vi.mocked(cleanupApi.saveSourceCredential).mockResolvedValue({
      ...acoustIdSource,
      configured: true,
      available: true,
      credential_saved: true,
      credential_source: "saved",
      key_hint: "••••1234",
      configuration_hint: "Encrypted in secure server storage.",
      unavailable_reason: null,
    });
    vi.mocked(cleanupApi.deleteSourceCredential).mockResolvedValue(acoustIdSource);
    vi.mocked(cleanupApi.batches).mockResolvedValue([]);
    vi.mocked(assistantProvidersApi.getStatus).mockResolvedValue(frameworkStatus);
    vi.mocked(assistantProvidersApi.initializeCredentialStorage).mockResolvedValue(
      frameworkStatus,
    );
  });

  it("loads and persists the active catalog-source policy", async () => {
    const user = userEvent.setup();
    render(
      <MemoryRouter>
        <LibraryCleanupSourcesView />
      </MemoryRouter>,
    );

    const toggle = await screen.findByRole("checkbox", { name: "Use in cleanup" });
    expect(toggle).toBeChecked();
    expect(screen.getByText("No API key required")).toBeVisible();

    await user.click(toggle);
    await waitFor(() =>
      expect(cleanupApi.updateSource).toHaveBeenCalledWith("musicbrainz", false),
    );
    expect(toggle).not.toBeChecked();
  });

  it("keeps source switches usable when encrypted-storage status cannot load", async () => {
    vi.mocked(assistantProvidersApi.getStatus).mockRejectedValue(
      new Error("status unavailable"),
    );

    render(
      <MemoryRouter>
        <LibraryCleanupSourcesView />
      </MemoryRouter>,
    );

    expect(
      await screen.findByText(
        "Encrypted key-storage status is unavailable. Existing source switches still work.",
      ),
    ).toBeVisible();
    expect(screen.getByRole("checkbox", { name: "Use in cleanup" })).toBeEnabled();
  });

  it("saves and removes a catalog key without returning it to the form", async () => {
    const user = userEvent.setup();
    const submittedCredential = "fixture";
    vi.mocked(cleanupApi.sources).mockResolvedValue([acoustIdSource]);
    vi.mocked(confirmDialog).mockResolvedValue(true);

    render(
      <MemoryRouter>
        <LibraryCleanupSourcesView />
      </MemoryRouter>,
    );

    const keyInput = await screen.findByLabelText("AcoustID API key");
    await user.type(keyInput, submittedCredential);
    await user.click(screen.getByRole("button", { name: "Save API key" }));

    await waitFor(() =>
      expect(cleanupApi.saveSourceCredential).toHaveBeenCalledWith(
        "acoustid",
        submittedCredential,
      ),
    );
    expect(keyInput).toHaveValue("");
    expect(screen.getByText("application API key saved · ••••1234")).toBeVisible();
    expect(screen.getByLabelText("Replace AcoustID API key")).toHaveValue("");

    await user.click(screen.getByRole("button", { name: "Remove saved key" }));
    expect(confirmDialog).toHaveBeenCalledWith({
      title: "Remove the saved AcoustID key?",
      body: expect.stringContaining("encrypted key will be deleted"),
      confirmLabel: "Remove API key",
      tone: "danger",
    });
    await waitFor(() =>
      expect(cleanupApi.deleteSourceCredential).toHaveBeenCalledWith("acoustid"),
    );
  });

  it("mounts server journals in the dedicated rollback tab", async () => {
    render(
      <MemoryRouter>
        <LibraryCleanupHistoryView />
      </MemoryRouter>,
    );

    expect(await screen.findByText("No cleanup runs yet")).toBeVisible();
    expect(screen.getByText("Revert from a downloaded journal file…")).toBeVisible();
  });
});
