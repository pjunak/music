import { render, screen, waitFor } from "@testing-library/react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";

import type * as ApiModule from "@/core/api";

vi.mock("@/core/ws", () => ({ wsClient: { send: vi.fn() } }));
vi.mock("@/core/api", async (importActual) => {
  const actual = await importActual<typeof ApiModule>();
  return {
    ...actual,
    presetsApi: { ...actual.presetsApi, list: vi.fn().mockResolvedValue([]) },
  };
});

import { usePlayerStore } from "@/core/playerStore";

import { PresetsSection } from "./PresetsSection";

beforeEach(() => {
  // The cold-load condition: no snapshot yet, so `state` is null. With an
  // unstable `?? []` selector this render loops to React #185 ("Maximum update
  // depth exceeded") and throws; a stable selector renders fine.
  usePlayerStore.setState({ state: null, myDeviceId: null });
});

afterEach(() => {
  usePlayerStore.setState({ state: null, myDeviceId: null });
});

describe("PresetsSection with null player state", () => {
  it("renders without an infinite update loop while the WS is still connecting", async () => {
    expect(() => render(<PresetsSection />)).not.toThrow();
    await waitFor(() =>
      expect(screen.getByText(/No presets installed/i)).toBeInTheDocument(),
    );
  });
});
