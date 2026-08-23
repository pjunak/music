import { act, render, screen, waitFor } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { MemoryRouter } from "react-router-dom";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";

import type * as ApiModule from "@/core/api";
import type {
  BackgroundJob,
  LibraryTagPage,
  ModelTaggingAvailability,
} from "@/core/api";
import { usePlayerStore } from "@/core/playerStore";
import type { PlayerState } from "@/core/types";

vi.mock("@/core/api", async (importActual) => {
  const actual = await importActual<typeof ApiModule>();
  return {
    ...actual,
    assistantApi: {
      ...actual.assistantApi,
      planModelTagging: vi.fn(),
      startModelTagging: vi.fn(),
      queryModelLibraryTags: vi.fn(),
      reviewAnalysisTagsBulk: vi.fn(),
    },
    jobsApi: {
      ...actual.jobsApi,
      list: vi.fn(),
      get: vi.fn(),
      cancel: vi.fn(),
    },
  };
});

vi.mock("@/components/confirmDialog", () => ({ confirmDialog: vi.fn() }));
vi.mock("@/core/toast", () => ({
  toast: { success: vi.fn(), error: vi.fn(), warn: vi.fn() },
}));
vi.mock("@/core/ws", () => ({ wsClient: { send: vi.fn() } }));

import { confirmDialog } from "@/components/confirmDialog";
import {
  MODEL_TAGGING_DISCLOSURE_VERSION,
  assistantApi,
  jobsApi,
} from "@/core/api";
import { wsClient } from "@/core/ws";

import { MoodTaggingDialog } from "./MoodTaggingDialog";

const availability: ModelTaggingAvailability = {
  available: true,
  reason_code: null,
  role_id: "music_tagger",
  connection_name: "DeepSeek direct",
  model_id: "deepseek-v4-pro",
  quality_evaluation_id: "music-tagging-quality-v1",
  job_kind: "assistant.model-music-tagging",
  library_tracks: 90,
  scope_tracks: 1,
  tracks_with_audio_evidence: 1,
  current_profiles: 0,
  tracks_needing_tags: 1,
  estimated_provider_requests: 1,
  disclosure: {
    version: MODEL_TAGGING_DISCLOSURE_VERSION,
    shared_with_provider: ["Library-relative paths"],
    never_shared: ["Audio files"],
    allowed_tags: ["forest", "calm"],
    tracks_per_request: 20,
    may_incur_cost: true,
  },
};

const succeededJob: BackgroundJob = {
  id: "mood-job-1",
  kind: "assistant.model-music-tagging",
  status: "succeeded",
  parameters: {},
  result: {
    schema_version: "assistant-model-music-tagging-job-result/v5",
    analyzer_id: "model-evidence-tagger/v5",
    vocabulary_fingerprint: "a".repeat(64),
    library_tracks: 90,
    scope_tracks: 1,
    updated_profiles: 1,
    unchanged_profiles: 0,
    skipped_changed_tracks: 0,
  },
  error: null,
  progress_current: 1,
  progress_total: 1,
  progress_phase: "Saving reviewable suggestions",
  progress_message: "Processed 1 of 1 tracks",
  attempts: 1,
  retry_of_id: null,
  created_at: "2026-08-23T10:00:00Z",
  updated_at: "2026-08-23T10:00:02Z",
  started_at: "2026-08-23T10:00:00Z",
  finished_at: "2026-08-23T10:00:02Z",
};

const reviewPage: LibraryTagPage = {
  total: 1,
  offset: 0,
  limit: 50,
  items: [
    {
      track_id: 9,
      path: "Campaign/Forest/Quiet Road.flac",
      title: "Quiet Road",
      display_title: "",
      artist: "Tabletop Ensemble",
      album: "Wilderness",
      manual_tags: [],
      analysis_analyzer: null,
      analysis_tags: [],
      analysis_confidence: null,
      audio_signal: null,
      analysis_suggestions: [
        {
          tag: "forest",
          analyzer_id: "model-evidence-tagger/v5",
          source_signature: "source-1",
          confidence: "high",
          evidence: ["Library path contains Forest."],
          status: "pending",
        },
        {
          tag: "calm",
          analyzer_id: "model-evidence-tagger/v5",
          source_signature: "source-1",
          confidence: "low",
          evidence: ["Bounded signal evidence is restrained."],
          status: "pending",
        },
      ],
    },
  ],
};

beforeEach(() => {
  vi.clearAllMocks();
  usePlayerStore.setState({ state: { active_mode_id: "dnd" } as PlayerState });
  vi.mocked(jobsApi.list).mockResolvedValue([]);
  vi.mocked(assistantApi.planModelTagging).mockResolvedValue(availability);
  vi.mocked(assistantApi.startModelTagging).mockResolvedValue(succeededJob);
  vi.mocked(assistantApi.queryModelLibraryTags)
    .mockResolvedValueOnce(reviewPage)
    .mockResolvedValue({ ...reviewPage, total: 0, items: [] });
  vi.mocked(assistantApi.reviewAnalysisTagsBulk).mockResolvedValue({
    requested_items: 2,
    applied: [],
    failures: [],
  });
  vi.mocked(confirmDialog).mockResolvedValue(true);
});

afterEach(() => {
  act(() => usePlayerStore.setState({ state: null }));
});

describe("MoodTaggingDialog", () => {
  it("keeps existing database suggestions reviewable when the model is unavailable", async () => {
    vi.mocked(assistantApi.planModelTagging).mockResolvedValue({
      ...availability,
      available: false,
      reason_code: "role_not_enabled",
    });
    const user = userEvent.setup();

    render(
      <MemoryRouter>
        <MoodTaggingDialog
          path="Campaign/Forest"
          checkedIds={[9]}
          onClose={vi.fn()}
          onChanged={vi.fn()}
        />
      </MemoryRouter>,
    );

    const review = await screen.findByRole("button", {
      name: "Review existing suggestions",
    });
    expect(screen.getByRole("button", { name: "Create suggestions" })).toBeDisabled();
    await user.click(review);
    expect(await screen.findByText("Quiet Road")).toBeInTheDocument();
  });

  it("reconnects to an active durable run when reopened", async () => {
    vi.mocked(jobsApi.list).mockResolvedValue([
      {
        ...succeededJob,
        status: "running",
        parameters: {
          scope: {
            type: "folder",
            path: "Campaign/Forest",
            recursive: true,
            track_ids: [],
          },
        },
        result: null,
        finished_at: null,
      },
    ]);

    render(
      <MemoryRouter>
        <MoodTaggingDialog
          path=""
          checkedIds={[]}
          onClose={vi.fn()}
          onChanged={vi.fn()}
        />
      </MemoryRouter>,
    );

    expect(
      await screen.findByRole("heading", { name: "Creating mood-tag suggestions" }),
    ).toBeInTheDocument();
    expect(
      screen.getByRole("button", { name: "Close and keep running" }),
    ).toBeInTheDocument();
  });

  it("runs the selected scope, auditions tracks, and explicitly accepts reviewed tags", async () => {
    const onChanged = vi.fn();
    const user = userEvent.setup();
    render(
      <MemoryRouter>
        <MoodTaggingDialog
          path="Campaign/Forest"
          checkedIds={[9]}
          onClose={vi.fn()}
          onChanged={onChanged}
        />
      </MemoryRouter>,
    );

    await waitFor(() =>
      expect(assistantApi.planModelTagging).toHaveBeenCalledWith({
        type: "tracks",
        track_ids: [9],
      }),
    );
    expect(screen.getByRole("radio", { name: /Selected tracks/ })).toBeChecked();
    await user.click(screen.getByRole("button", { name: "Create suggestions" }));

    expect(confirmDialog).toHaveBeenCalledWith(
      expect.objectContaining({ title: "Create mood-library suggestions?" }),
    );
    await waitFor(() =>
      expect(assistantApi.startModelTagging).toHaveBeenCalledWith(
        false,
        MODEL_TAGGING_DISCLOSURE_VERSION,
        { type: "tracks", track_ids: [9] },
      ),
    );

    expect(await screen.findByText("Quiet Road")).toBeInTheDocument();
    expect(screen.getByRole("checkbox", { name: "Select forest (high confidence)" })).toBeChecked();
    expect(screen.getByRole("checkbox", { name: "Select calm (low confidence)" })).not.toBeChecked();

    await user.click(screen.getByRole("button", { name: "Play Quiet Road" }));
    expect(vi.mocked(wsClient.send).mock.calls.map(([action]) => action)).toEqual([
      { type: "ambient_stop" },
      { type: "ambient_play_track", track_id: 9 },
    ]);

    await user.click(
      screen.getByRole("checkbox", { name: "Select calm (low confidence)" }),
    );
    await user.click(screen.getByRole("button", { name: "Add 2 to mood library" }));

    await waitFor(() =>
      expect(assistantApi.reviewAnalysisTagsBulk).toHaveBeenCalledWith(
        expect.arrayContaining([
          expect.objectContaining({ track_id: 9, tag: "forest" }),
          expect.objectContaining({ track_id: 9, tag: "calm" }),
        ]),
        "accepted",
      ),
    );
    expect(onChanged).toHaveBeenCalled();
  });
});
