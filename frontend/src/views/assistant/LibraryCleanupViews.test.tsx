import { render, screen, waitFor } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { MemoryRouter } from "react-router-dom";
import { beforeEach, describe, expect, it, vi } from "vitest";

import type * as ApiModule from "@/core/api";
import type * as ProviderApiModule from "@/core/assistantProvidersApi";
import type { CleanupSource } from "@/core/api";
import type { ModelRole } from "@/core/assistantProvidersApi";

vi.mock("@/core/api", async (importActual) => {
  const actual = await importActual<typeof ApiModule>();
  return {
    ...actual,
    cleanupApi: {
      ...actual.cleanupApi,
      sources: vi.fn(),
      updateSource: vi.fn(),
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
      listRoles: vi.fn(),
    },
  };
});

vi.mock("@/core/toast", () => ({
  toast: { success: vi.fn(), error: vi.fn(), warn: vi.fn() },
}));

import { cleanupApi } from "@/core/api";
import { assistantProvidersApi } from "@/core/assistantProvidersApi";

import {
  LibraryCleanupHistoryView,
  LibraryCleanupModelView,
  LibraryCleanupSourcesView,
} from "./LibraryCleanupViews";

const musicBrainzSource: CleanupSource = {
  id: "musicbrainz",
  label: "MusicBrainz",
  description: "Checks ambiguous names.",
  enabled: true,
  capabilities: ["artist_name_verification", "album_name_verification"],
  credential_kind: null,
};

const cleanupRole: ModelRole = {
  role_id: "library_cleanup",
  label: "Library cleanup",
  description: "Reserved for a review-first cleanup model pass.",
  required_capability_ids: ["structured_text"],
  configuration_available: false,
  connection_id: null,
  connection_name: null,
  model_id: "",
  enabled: false,
  effective_enabled: false,
  timeout_seconds: 30,
  max_output_tokens: 1000,
  thinking_mode: "provider_default",
  verification_status: null,
  conformance_status: "never",
  conformance_error_code: null,
  last_conformance_at: null,
  updated_at: null,
};

describe("Library cleanup workspace views", () => {
  beforeEach(() => {
    vi.clearAllMocks();
    vi.mocked(cleanupApi.sources).mockResolvedValue([musicBrainzSource]);
    vi.mocked(cleanupApi.updateSource).mockResolvedValue({
      ...musicBrainzSource,
      enabled: false,
    });
    vi.mocked(cleanupApi.batches).mockResolvedValue([]);
    vi.mocked(assistantProvidersApi.listRoles).mockResolvedValue([cleanupRole]);
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

  it("mounts the unfinished cleanup role beside the tool instead of pretending it is active", async () => {
    render(
      <MemoryRouter>
        <LibraryCleanupModelView />
      </MemoryRouter>,
    );

    expect(await screen.findByRole("heading", { name: "Library cleanup" })).toBeVisible();
    expect(screen.getByText("planned")).toBeVisible();
    expect(screen.getByText("Local cleanup stays authoritative")).toBeVisible();
    expect(screen.getByText("Not assigned")).toBeVisible();
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
