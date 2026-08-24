import { render, screen } from "@testing-library/react";
import { beforeEach, describe, expect, it, vi } from "vitest";

import type * as ApiModule from "@/core/api";

vi.mock("@/core/api", async (importActual) => {
  const actual = await importActual<typeof ApiModule>();
  return {
    ...actual,
    authApi: {
      ...actual.authApi,
      listSessions: vi.fn(),
    },
    devicesApi: {
      ...actual.devicesApi,
      list: vi.fn(),
    },
  };
});

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
