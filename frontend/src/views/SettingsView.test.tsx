import { render, screen, waitFor } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { beforeEach, describe, expect, it, vi } from "vitest";

import type * as ApiModule from "@/core/api";

vi.mock("@/core/api", async (importActual) => {
  const actual = await importActual<typeof ApiModule>();
  return {
    ...actual,
    authApi: {
      ...actual.authApi,
      listSessions: vi.fn(),
      revokeSession: vi.fn(),
    },
    devicesApi: {
      ...actual.devicesApi,
      list: vi.fn(),
    },
  };
});

vi.mock("@/components/confirmDialog", () => ({ confirmDialog: vi.fn() }));
vi.mock("@/core/toast", () => ({ toast: { success: vi.fn(), error: vi.fn() } }));

import { confirmDialog } from "@/components/confirmDialog";
import { authApi, devicesApi } from "@/core/api";
import { useAuthStore } from "@/core/auth";

import { SettingsView } from "./SettingsView";

beforeEach(() => {
  vi.clearAllMocks();
  vi.mocked(devicesApi.list).mockResolvedValue([]);
  vi.mocked(authApi.listSessions).mockResolvedValue([]);
  useAuthStore.setState({
    status: "authenticated",
    user: { id: 1, username: "operator" },
  });
});

describe("SettingsView", () => {
  it("revokes the complete management ID while protecting the current session", async () => {
    const currentId = "a".repeat(32);
    const otherId = "b".repeat(32);
    vi.mocked(authApi.listSessions).mockResolvedValue([
      { session_id: currentId, token_prefix: currentId, created_at: "2026-09-05T12:00:00Z", expires_at: "2026-10-05T12:00:00Z", last_seen: "2026-09-05T12:00:00Z", is_current: true },
      { session_id: otherId, token_prefix: otherId, created_at: "2026-09-05T11:00:00Z", expires_at: "2026-10-05T11:00:00Z", last_seen: "2026-09-05T11:00:00Z", is_current: false },
    ]);
    vi.mocked(confirmDialog).mockResolvedValue(true);
    vi.mocked(authApi.revokeSession).mockResolvedValue(undefined);
    render(<SettingsView />);
    const buttons = await screen.findAllByRole("button", { name: "Revoke" });
    expect(buttons[0]).toBeDisabled();
    await userEvent.click(buttons[1]);
    await waitFor(() => expect(authApi.revokeSession).toHaveBeenCalledExactlyOnceWith(otherId));
    expect(authApi.revokeSession).not.toHaveBeenCalledWith(otherId.slice(0, 12));
  });

  it("presents desktop settings as one structured workspace", async () => {
    const { container } = render(<SettingsView />);

    expect(
      screen.getByRole("heading", { level: 1, name: "Settings" }),
    ).toBeInTheDocument();
    expect(
      screen.getAllByRole("heading", { level: 2 }).map((heading) => heading.textContent),
    ).toEqual([
      "Display",
      "Account",
      "Devices",
      "Active sessions",
      "Backup",
      "Diagnostics",
    ]);
    expect(container.querySelector(".settings-grid")?.children).toHaveLength(6);
    expect(await screen.findByText("No active sessions.")).toBeInTheDocument();
  });
});
