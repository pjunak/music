import { render, screen, waitFor } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { beforeEach, describe, expect, it, vi } from "vitest";

import type * as ApiModule from "@/core/api";
import type { ManualTagCatalog } from "@/core/api";

vi.mock("@/core/api", async (importActual) => {
  const actual = await importActual<typeof ApiModule>();
  return {
    ...actual,
    assistantApi: {
      ...actual.assistantApi,
      renameManualTag: vi.fn(),
      previewTagCleanup: vi.fn(),
      applyTagCleanup: vi.fn(),
    },
  };
});

vi.mock("@/components/inputDialog", () => ({ inputDialog: vi.fn() }));
vi.mock("@/components/confirmDialog", () => ({ confirmDialog: vi.fn() }));
vi.mock("@/core/toast", () => ({
  toast: { success: vi.fn(), error: vi.fn() },
}));

import { confirmDialog } from "@/components/confirmDialog";
import { inputDialog } from "@/components/inputDialog";
import { assistantApi } from "@/core/api";
import { toast } from "@/core/toast";

import { TagCatalogManager } from "./TagCatalogManager";

const catalog: ManualTagCatalog = {
  starter_groups: [],
  used_tags: ["medieval", "tavern"],
  tag_usage: [
    { tag: "medieval", track_count: 3 },
    { tag: "tavern", track_count: 2 },
  ],
};

beforeEach(() => {
  vi.clearAllMocks();
});

describe("TagCatalogManager", () => {
  it("requires explicit cleanup selection and applies only checked suggestions", async () => {
    vi.mocked(assistantApi.previewTagCleanup).mockResolvedValue({
      schema_version: "assistant-tag-cleanup-preview/v2",
      catalog_signature: "a".repeat(64),
      vocabulary_fingerprint: "f".repeat(64),
      suggestions: [
        {
          id: "1".repeat(64),
          source: "medival",
          target: "medieval",
          reason_code: "vocabulary_typo",
          reason: "One clear spelling edit from a canonical tag.",
          source_track_count: 2,
          target_track_count: 3,
          merged: true,
        },
        {
          id: "2".repeat(64),
          source: "taverns",
          target: "tavern",
          reason_code: "vocabulary_plural",
          reason: "Matches the plural form of a canonical tag.",
          source_track_count: 1,
          target_track_count: 0,
          merged: false,
        },
      ],
    });
    vi.mocked(confirmDialog).mockResolvedValue(true);
    vi.mocked(assistantApi.applyTagCleanup).mockResolvedValue({
      schema_version: "assistant-tag-cleanup-apply/v1",
      requested_items: 1,
      applied: [
        {
          source: "medival",
          target: "medieval",
          affected_tracks: 2,
          merged: true,
        },
      ],
      catalog_signature: "b".repeat(64),
    });
    const onChanged = vi.fn();
    const user = userEvent.setup();
    render(<TagCatalogManager catalog={catalog} onChanged={onChanged} />);

    await user.click(screen.getByText("Manage used tags"));
    await user.click(screen.getByRole("button", { name: "Find suggestions" }));
    const applyButton = await screen.findByRole("button", {
      name: "Apply selected",
    });
    expect(applyButton).toBeDisabled();

    await user.click(screen.getByText(/medival/));
    expect(applyButton).toBeEnabled();
    await user.click(applyButton);

    await waitFor(() =>
      expect(assistantApi.applyTagCleanup).toHaveBeenCalledWith(
        "a".repeat(64),
        "f".repeat(64),
        [{ source: "medival", target: "medieval" }],
      ),
    );
    expect(confirmDialog).toHaveBeenCalledWith(
      expect.objectContaining({
        title: "Apply selected mood-tag cleanup?",
        confirmLabel: "Apply selected",
      }),
    );
    expect(onChanged).toHaveBeenCalledOnce();
  });

  it("confirms and merges a renamed tag into an existing tag", async () => {
    vi.mocked(inputDialog).mockResolvedValue("medieval");
    vi.mocked(confirmDialog).mockResolvedValue(true);
    vi.mocked(assistantApi.renameManualTag).mockResolvedValue({
      source: "tavern",
      target: "medieval",
      affected_tracks: 2,
      merged: true,
    });
    const onChanged = vi.fn();
    const user = userEvent.setup();
    render(<TagCatalogManager catalog={catalog} onChanged={onChanged} />);

    await user.click(screen.getByText("Manage used tags"));
    await user.click(
      screen.getByRole("button", { name: "Rename or merge tavern" }),
    );

    await waitFor(() =>
      expect(assistantApi.renameManualTag).toHaveBeenCalledWith("tavern", "medieval"),
    );
    expect(confirmDialog).toHaveBeenCalledWith(
      expect.objectContaining({ title: "Merge these tags?", confirmLabel: "Merge tags" }),
    );
    expect(toast.success).toHaveBeenCalledWith("Tags merged", "2 tracks were updated.");
    expect(onChanged).toHaveBeenCalledOnce();
  });
});
