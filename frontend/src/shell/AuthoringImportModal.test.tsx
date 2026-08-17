import { render, screen, waitFor, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { beforeEach, describe, expect, it, vi } from "vitest";

import type * as ApiModule from "@/core/api";
import type { AuthoringImportPreview } from "@/core/api";
import type { ModeSummary } from "@/core/types";

vi.mock("@/core/api", async (importActual) => {
  const actual = await importActual<typeof ApiModule>();
  return {
    ...actual,
    modesApi: { ...actual.modesApi, list: vi.fn() },
    authoringImportApi: {
      previewMode: vi.fn(),
      commitMode: vi.fn(),
      previewDocument: vi.fn(),
      commitDocument: vi.fn(),
    },
  };
});

vi.mock("@/core/toast", () => ({
  toast: {
    success: vi.fn(),
    warn: vi.fn(),
  },
}));

import { authoringImportApi, modesApi } from "@/core/api";
import { toast } from "@/core/toast";

import { AuthoringImportModal } from "./AuthoringImportModal";

function mode(id: string, name: string): ModeSummary {
  return {
    id,
    name,
    panels: [],
    playlist_categories: [],
    has_theme: false,
    default_crossfade_ms: 1200,
    default_soundboard: null,
  };
}

const modePreview: AuthoringImportPreview = {
  source: { type: "mode", id: "source", name: "Source mode" },
  target_mode: { id: "target", name: "Target mode" },
  items: [
    {
      kind: "preset",
      resource_id: "cave",
      name: "Cave",
      summary: "1 effect",
      status: "ready",
      reason: null,
      issues: [],
    },
    {
      kind: "cue",
      resource_id: "arrival",
      name: "Arrival",
      summary: "2 actions",
      status: "ready",
      reason: null,
      issues: [
        {
          code: "dependency_selection_required",
          severity: "warning",
          message: "Also select EQ preset 'cave'.",
          related_item: { kind: "preset", resource_id: "cave" },
        },
      ],
    },
    {
      kind: "playlist",
      resource_id: "1",
      name: "Town",
      summary: "4 tracks",
      status: "conflict",
      reason: "A playlist named Town already exists in the target mode.",
      issues: [
        {
          code: "target_conflict",
          severity: "error",
          message: "A playlist named Town already exists in the target mode.",
          related_item: null,
        },
      ],
    },
  ],
};

const documentPreview: AuthoringImportPreview = {
  source: {
    type: "document",
    id: "authoring-import/v1",
    name: "Assistant draft",
  },
  target_mode: { id: "target", name: "Target mode" },
  items: [
    {
      kind: "playlist",
      resource_id: "0",
      name: "Night Walk",
      summary: "2 tracks · exploration",
      status: "ready",
      reason: null,
      issues: [
        {
          code: "missing_tracks",
          severity: "warning",
          message: "1 track reference is unavailable and will be omitted.",
          related_item: null,
        },
      ],
    },
    {
      kind: "preset",
      resource_id: "broken",
      name: "Broken effect",
      summary: "1 effect",
      status: "invalid",
      reason: "Unknown effect type.",
      issues: [
        {
          code: "unsupported_effect",
          severity: "error",
          message: "Unknown effect type.",
          related_item: null,
        },
      ],
    },
  ],
};

beforeEach(() => {
  vi.clearAllMocks();
  vi.mocked(modesApi.list).mockResolvedValue([
    mode("target", "Target mode"),
    mode("source", "Source mode"),
  ]);
  vi.mocked(authoringImportApi.previewMode).mockResolvedValue(modePreview);
  vi.mocked(authoringImportApi.previewDocument).mockResolvedValue(documentPreview);
  vi.mocked(authoringImportApi.commitMode).mockResolvedValue({
    imported: [modePreview.items[0]],
    skipped: [],
    missing_track_paths: [],
  });
  vi.mocked(authoringImportApi.commitDocument).mockResolvedValue({
    imported: [documentPreview.items[0]],
    skipped: [],
    missing_track_paths: ["missing.flac"],
  });
});

describe("AuthoringImportModal", () => {
  it("previews mode conflicts and commits only the selected resources", async () => {
    const user = userEvent.setup();
    const onClose = vi.fn();
    const onImported = vi.fn();

    render(
      <AuthoringImportModal
        open
        targetModeId="target"
        onClose={onClose}
        onImported={onImported}
      />,
    );

    expect(await screen.findByRole("combobox", { name: "Source mode" })).toHaveValue(
      "source",
    );
    await waitFor(() =>
      expect(authoringImportApi.previewMode).toHaveBeenCalledWith("source", "target"),
    );

    const direction = await screen.findByLabelText("Import direction");
    expect(within(direction).getByText("Source mode")).toBeInTheDocument();
    expect(within(direction).getByText("Target mode")).toBeInTheDocument();
    expect(screen.getByRole("checkbox", { name: /Cave/ })).toBeChecked();
    expect(screen.getByRole("checkbox", { name: /Arrival/ })).toBeChecked();
    expect(screen.getByRole("checkbox", { name: /Town/ })).toBeDisabled();
    expect(screen.getByRole("button", { name: "Import 2 items" })).toBeEnabled();

    await user.click(screen.getByRole("checkbox", { name: /Arrival/ }));
    await user.click(screen.getByRole("button", { name: "Import 1 item" }));

    await waitFor(() =>
      expect(authoringImportApi.commitMode).toHaveBeenCalledWith(
        "source",
        "target",
        [{ kind: "preset", resource_id: "cave" }],
      ),
    );
    expect(toast.success).toHaveBeenCalledWith("Authoring imported", "1 item imported");
    expect(onImported).toHaveBeenCalled();
    expect(onClose).toHaveBeenCalledOnce();
  });

  it("blocks a selection when a required related item is deselected", async () => {
    const user = userEvent.setup();
    render(
      <AuthoringImportModal
        open
        targetModeId="target"
        onClose={vi.fn()}
        onImported={vi.fn()}
      />,
    );
    await screen.findByRole("checkbox", { name: /Arrival/ });
    await user.click(screen.getByRole("checkbox", { name: /Cave/ }));

    expect(
      screen.getByRole("button", { name: "Select required items" }),
    ).toBeDisabled();
    expect(screen.getByRole("alert")).toHaveTextContent(
      "1 required related item is not selected",
    );
  });

  it("reads, reviews, and commits a JSON file", async () => {
    const user = userEvent.setup();
    const document = {
      schema: "authoring-import/v1",
      name: "Assistant draft",
      playlists: [{ name: "Night Walk", tracks: ["Music/night.flac"] }],
    };
    render(
      <AuthoringImportModal
        open
        targetModeId="target"
        onClose={vi.fn()}
        onImported={vi.fn()}
      />,
    );

    await user.click(screen.getByRole("radio", { name: /JSON file/ }));
    await user.upload(
      screen.getByLabelText("Authoring JSON document"),
      new File([JSON.stringify(document)], "assistant-draft.json", {
        type: "application/json",
      }),
    );

    await waitFor(() =>
      expect(authoringImportApi.previewDocument).toHaveBeenCalledWith(
        "target",
        document,
        "assistant-draft.json",
      ),
    );
    expect(screen.getByRole("checkbox", { name: /Night Walk/ })).toBeChecked();
    expect(screen.getByRole("checkbox", { name: /Broken effect/ })).toBeDisabled();
    expect(screen.getByText("Unknown effect type.")).toBeInTheDocument();

    await user.click(screen.getByRole("button", { name: "Import 1 item" }));
    await waitFor(() =>
      expect(authoringImportApi.commitDocument).toHaveBeenCalledWith(
        "target",
        document,
        [{ kind: "playlist", resource_id: "0" }],
        "assistant-draft.json",
      ),
    );
    expect(toast.warn).toHaveBeenCalledWith(
      "Authoring import completed with skips",
      "1 item imported · 1 missing tracks omitted",
    );
  });

  it("validates pasted JSON before sending it to the server", async () => {
    const user = userEvent.setup();
    render(
      <AuthoringImportModal
        open
        targetModeId="target"
        onClose={vi.fn()}
        onImported={vi.fn()}
      />,
    );

    await user.click(screen.getByRole("radio", { name: /Paste JSON/ }));
    await user.type(screen.getByLabelText("Authoring JSON"), "not JSON");
    await user.click(screen.getByRole("button", { name: "Review JSON" }));

    expect(await screen.findByRole("alert")).toHaveTextContent("not valid JSON");
    expect(authoringImportApi.previewDocument).not.toHaveBeenCalled();
  });

  it("keeps file and paste imports available when no other mode exists", async () => {
    vi.mocked(modesApi.list).mockResolvedValue([mode("target", "Target mode")]);
    render(
      <AuthoringImportModal
        open
        targetModeId="target"
        onClose={vi.fn()}
        onImported={vi.fn()}
      />,
    );

    expect(
      await screen.findByText(/There is no other mode to copy from/),
    ).toBeInTheDocument();
    expect(screen.getByRole("radio", { name: /JSON file/ })).toBeEnabled();
    expect(screen.getByRole("radio", { name: /Paste JSON/ })).toBeEnabled();
    expect(authoringImportApi.previewMode).not.toHaveBeenCalled();
  });
});
