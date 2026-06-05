import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";

import type * as ApiModule from "@/core/api";

// Keep the real ApiError (refresh branches on `instanceof ApiError`); stub the
// network calls.
vi.mock("@/core/api", async (importActual) => {
  const actual = await importActual<typeof ApiModule>();
  return { ...actual, api: { ...actual.api, get: vi.fn(), post: vi.fn() } };
});

import { ApiError, api } from "@/core/api";
import { useAuthStore } from "@/core/auth";

const get = vi.mocked(api.get);

beforeEach(() => {
  vi.clearAllMocks();
  vi.spyOn(console, "warn").mockImplementation(() => {});
  useAuthStore.setState({ status: "unknown", user: null });
});

afterEach(() => {
  vi.restoreAllMocks();
});

describe("auth refresh resolution", () => {
  it("resolves the initial (unknown) check to anonymous on a non-401 failure", async () => {
    // The black-screen-forever bug: a 502 while the container boots must not
    // leave the app stranded on a blocking spinner.
    get.mockRejectedValueOnce(new ApiError(502, "bad gateway"));
    await useAuthStore.getState().refresh();
    expect(useAuthStore.getState().status).toBe("anonymous");
  });

  it("resolves the initial check to anonymous on a network error too", async () => {
    get.mockRejectedValueOnce(new TypeError("Failed to fetch"));
    await useAuthStore.getState().refresh();
    expect(useAuthStore.getState().status).toBe("anonymous");
  });

  it("treats a 401 as definitively anonymous", async () => {
    get.mockRejectedValueOnce(new ApiError(401, "unauthorized"));
    await useAuthStore.getState().refresh();
    expect(useAuthStore.getState().status).toBe("anonymous");
  });

  it("keeps an already-authenticated session through a transient non-401 blip", async () => {
    useAuthStore.setState({ status: "authenticated", user: { id: 1, username: "dm" } });
    get.mockRejectedValueOnce(new ApiError(503, "service unavailable"));
    await useAuthStore.getState().refresh();
    expect(useAuthStore.getState().status).toBe("authenticated");
    expect(useAuthStore.getState().user).toEqual({ id: 1, username: "dm" });
  });

  it("resolves to authenticated when /api/auth/me succeeds", async () => {
    get.mockResolvedValueOnce({ id: 7, username: "petr" });
    await useAuthStore.getState().refresh();
    expect(useAuthStore.getState().status).toBe("authenticated");
    expect(useAuthStore.getState().user).toEqual({ id: 7, username: "petr" });
  });
});
