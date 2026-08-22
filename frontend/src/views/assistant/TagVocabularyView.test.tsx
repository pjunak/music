import { render, screen } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { beforeEach, describe, expect, it, vi } from "vitest";

import type * as ApiModule from "@/core/api";
import type { ManualTagCatalog, TagVocabulary } from "@/core/api";

vi.mock("@/components/inputDialog", () => ({
  inputDialog: vi.fn(),
}));

vi.mock("@/core/api", async (importActual) => {
  const actual = await importActual<typeof ApiModule>();
  return {
    ...actual,
    assistantApi: {
      ...actual.assistantApi,
      getTagVocabulary: vi.fn(),
      getManualTagCatalog: vi.fn(),
      updateTagVocabulary: vi.fn(),
    },
  };
});

vi.mock("./ModelTagCleanupPanel", () => ({
  ModelTagCleanupPanel: () => <div>Cleanup pipeline</div>,
}));

vi.mock("./TagCatalogManager", () => ({
  TagCatalogManager: () => <div>Used-tag tools</div>,
}));

vi.mock("@/core/toast", () => ({
  toast: { success: vi.fn(), error: vi.fn() },
}));

import { assistantApi } from "@/core/api";
import { inputDialog } from "@/components/inputDialog";

import { TagVocabularyView } from "./TagVocabularyView";

const vocabulary: TagVocabulary = {
  schema_version: "assistant-tag-vocabulary/v1",
  revision: 4,
  fingerprint: "a".repeat(64),
  groups: [
    {
      key: "setting",
      label: "Setting",
      description: "Where the scene takes place.",
      tags: [
        {
          id: "setting.tavern",
          name: "tavern",
          description: "An inn, common room, or alehouse.",
          aliases: ["inn", "pub"],
        },
      ],
    },
    {
      key: "mood",
      label: "Mood",
      description: "The emotional tone.",
      tags: [
        {
          id: "mood.calm",
          name: "calm",
          description: "Peaceful and emotionally settled.",
          aliases: [],
        },
      ],
    },
  ],
};

const catalog: ManualTagCatalog = {
  starter_groups: [],
  used_tags: ["tavern", "wondrous"],
  tag_usage: [
    { tag: "tavern", track_count: 8 },
    { tag: "wondrous", track_count: 3 },
  ],
};

beforeEach(() => {
  vi.clearAllMocks();
  vi.mocked(assistantApi.getTagVocabulary).mockResolvedValue(vocabulary);
  vi.mocked(assistantApi.getManualTagCatalog).mockResolvedValue(catalog);
  vi.mocked(inputDialog).mockResolvedValue(null);
  vi.mocked(assistantApi.updateTagVocabulary).mockImplementation(
    async (_revision, groups) => ({
      ...vocabulary,
      revision: 5,
      fingerprint: "b".repeat(64),
      groups,
    }),
  );
});

describe("TagVocabularyView", () => {
  it("creates a stable ID from a new canonical tag name", async () => {
    vi.mocked(inputDialog).mockResolvedValueOnce("Heroic Arrival");
    const user = userEvent.setup();
    render(<TagVocabularyView />);

    await screen.findByRole("heading", { name: "Setting" });
    await user.click(screen.getByRole("button", { name: "Add setting tag" }));
    await user.click(screen.getByRole("button", { name: "Save vocabulary" }));

    expect(assistantApi.updateTagVocabulary).toHaveBeenCalledWith(
      4,
      expect.arrayContaining([
        expect.objectContaining({
          key: "setting",
          tags: expect.arrayContaining([
            expect.objectContaining({
              id: "setting.heroic-arrival",
              name: "heroic arrival",
            }),
          ]),
        }),
      ]),
    );
  });

  it("edits canonical definitions and promotes an existing manual tag", async () => {
    const user = userEvent.setup();
    render(<TagVocabularyView />);

    expect(await screen.findByRole("heading", { name: "Setting" })).toBeVisible();
    expect(screen.getByText("setting.tavern")).toBeVisible();
    expect(screen.getByText("wondrous")).toBeVisible();

    const description = screen.getByDisplayValue(
      "An inn, common room, or alehouse.",
    );
    await user.clear(description);
    await user.type(description, "A social inn or alehouse scene.");
    await user.selectOptions(
      screen.getByRole("combobox", { name: "Group for wondrous" }),
      "mood",
    );
    await user.click(screen.getByRole("button", { name: "Add to vocabulary" }));
    await user.click(screen.getByRole("button", { name: "Save vocabulary" }));

    expect(assistantApi.updateTagVocabulary).toHaveBeenCalledWith(
      4,
      expect.arrayContaining([
        expect.objectContaining({
          key: "setting",
          tags: [
            expect.objectContaining({
              id: "setting.tavern",
              description: "A social inn or alehouse scene.",
            }),
          ],
        }),
        expect.objectContaining({
          key: "mood",
          tags: expect.arrayContaining([
            expect.objectContaining({
              id: "mood.wondrous",
              name: "wondrous",
            }),
          ]),
        }),
      ]),
    );
  });
});
