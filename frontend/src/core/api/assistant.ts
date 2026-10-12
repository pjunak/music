import type * as Http from "@/generated/http";

import { requireCurrentTagPage, requireCurrentTagTrack } from "../analysisResponse";

import { api } from "./http";
import type { BackgroundJob } from "./jobs";

export type PlaylistSuggestionIntent = Http.PlaylistIntent;

export type PlaylistEnergyCurve = "steady" | "rising" | "falling" | "arc";

export type PlaylistSuggestionPlan = Http.PlaylistPlan;

export type PlaylistSuggestionCandidate = Http.PlaylistCandidate;

export type PlaylistSuggestion = Http.PlaylistSuggestionResponse;

export type PlaylistSuggestionRequest = Http.PlaylistSuggestionRequest;

export const MODEL_PLAYLIST_DISCLOSURE_VERSION =
  "assistant-playlist-model-disclosure/v4" as const;

export type ModelPlaylistDisclosure = Http.ModelPlaylistDisclosure;

export type ModelPlaylistAvailability = Http.ModelPlaylistAvailability;

export const MODEL_EQ_DISCLOSURE_VERSION =
  "assistant-eq-draft-disclosure/v2" as const;

export type ModelEqDisclosure = Http.ModelEqDisclosure;

export type ModelEqAvailability = Http.ModelEqAvailability;

export interface EqPresetDraft {
  name: string;
  goal: string;
  bands: Array<{ frequency: number; gain: number }>;
  rationale: string;
  cautions: string[];
}

export const MODEL_TAGGING_DISCLOSURE_VERSION =
  "assistant-model-music-tagging-disclosure/v16" as const;

export type ModelTaggingScope =
  | { type: "all" }
  | { type: "folder"; path: string; recursive: boolean }
  | { type: "tracks"; track_ids: number[] };

export type ModelTaggingContextPolicy = "include" | "skip";

export type ModelTaggingDisclosure = Http.ModelTaggingDisclosure;

export type ModelTaggingLimits = Http.ModelTaggingLimits;

export const DEFAULT_MODEL_TAGGING_LIMITS: ModelTaggingLimits = {
  max_tracks: 20, max_requests: 10, max_token_reservation: 1_000_000, stop_on_empty_batch: true,
};

export interface ModelBatchStatus {
  id: string; state: string; input_file_id: string | null;
  remote_batch_id: string | null; result: Record<string, unknown> | null;
}

export type ModelTaggingAvailability = Http.ModelTaggingAvailability;

export type LibraryContextSummary = Http.LibraryContextSummary;

export type LibraryContextPassSummary = Http.LibraryContextPassSummary;

export type TrackContextDetail = Http.TrackContextDetail;

export type StarterTagGroup = Http.StarterTagGroupOut;

export type ManualTagCatalog = Http.ManualTagCatalog;

export type TagVocabularyEntry = Http.TagVocabularyEntry;

export type TagVocabularyGroup = Http.TagVocabularyGroup;

export type TagVocabulary = Http.TagVocabularyOut;

export type ManualTagUsage = Http.ManualTagUsage;

export type BulkManualTagFailure = Http.BulkManualTagFailure;

export type BulkManualTagResult = Http.BulkManualTagResult;

export type ManualTagRenameResult = Http.ManualTagRenameResult;

export type AnalysisTagReviewDecision = "pending" | "accepted" | "rejected";

export type AnalysisTagSuggestion = Http.AnalysisTagSuggestionOut;

export type AnalysisTagReviewResult = Http.AnalysisTagReviewResult;

export type BulkAnalysisTagReviewDecision = Exclude<
  AnalysisTagReviewDecision,
  "pending"
>;

export type AnalysisTagReviewTarget = Http.BulkAnalysisTagReviewItem;

export interface BulkAnalysisTagReviewApplied extends AnalysisTagReviewTarget {
  decision: BulkAnalysisTagReviewDecision;
}

export interface BulkAnalysisTagReviewFailure extends AnalysisTagReviewTarget {
  code: "not_found" | "stale" | "tag_limit";
  error: string;
}

export type BulkAnalysisTagReviewResult = Http.BulkAnalysisTagReviewResult;

export type LibraryTagTrack = Http.LibraryTagTrack;

export type ModelTagFilter = "processed" | "current" | "stale" | "missing" | "with_suggestions" | "without_suggestions";
export type TagSuggestionSource = "model" | "catalog";

export type TagReviewSummary = Http.TagReviewSummary;

export type LibraryTagPage = Http.LibraryTagPage;

export const assistantApi = {
  suggestPlaylist: (payload: PlaylistSuggestionRequest) =>
    api.post<PlaylistSuggestion>("/api/assistant/playlists/suggest", payload),
  getModelPlaylistAvailability: () =>
    api.get<ModelPlaylistAvailability>("/api/assistant/playlists/model-status"),
  startModelPlaylistSuggestion: (
    payload: PlaylistSuggestionRequest,
    disclosureVersion: typeof MODEL_PLAYLIST_DISCLOSURE_VERSION,
  ) =>
    api.post<BackgroundJob>("/api/assistant/playlists/model-suggestions/jobs", {
      request: payload,
      disclosure_version: disclosureVersion,
      consent: true,
    }),
  getModelEqAvailability: () =>
    api.get<ModelEqAvailability>("/api/assistant/eq/model-status"),
  startModelEqDraft: (
    request: { name: string; goal: string },
    disclosureVersion: typeof MODEL_EQ_DISCLOSURE_VERSION,
  ) =>
    api.post<BackgroundJob>("/api/assistant/eq/drafts/jobs", {
      request,
      disclosure_version: disclosureVersion,
      consent: true,
    }),
  startLibraryContextAnalysis: (
    force = false,
    scope: ModelTaggingScope = { type: "all" },
  ) =>
    api.post<BackgroundJob>("/api/assistant/library-context/jobs", {
      force,
      scope,
    }),
  getLibraryContextSummary: () =>
    api.get<LibraryContextSummary>("/api/assistant/library-context/summary"),
  getTrackContext: (trackId: number) =>
    api.get<TrackContextDetail>(
      `/api/assistant/library-context/tracks/${encodeURIComponent(trackId)}`,
    ),
  planModelTagging: (
    scope: ModelTaggingScope,
    contextPolicy: ModelTaggingContextPolicy = "include",
    force = false,
    limits: ModelTaggingLimits = DEFAULT_MODEL_TAGGING_LIMITS,
    executionMode: "standard" | "batch" = "standard",
  ) =>
    api.post<ModelTaggingAvailability>(
      "/api/assistant/library-tags/model-plan",
      { scope, context_policy: contextPolicy, force, limits, execution_mode: executionMode },
    ),
  startModelTagging: (
    force: boolean,
    disclosureVersion: typeof MODEL_TAGGING_DISCLOSURE_VERSION,
    scope: ModelTaggingScope = { type: "all" },
    contextPolicy: ModelTaggingContextPolicy = "include",
    limits: ModelTaggingLimits = DEFAULT_MODEL_TAGGING_LIMITS,
    executionMode: "standard" | "batch" = "standard",
  ) =>
    api.post<BackgroundJob>("/api/assistant/library-tags/model-jobs", {
      limits,
      execution_mode: executionMode,
      force,
      scope,
      context_policy: contextPolicy,
      disclosure_version: disclosureVersion,
      consent: true,
    }),
  getModelBatch: (id: string) => api.get<ModelBatchStatus>(`/api/assistant/library-tags/model-batches/${encodeURIComponent(id)}`),
  updateModelBatch: (id: string, cancel: boolean, remoteBatchId?: string, abandonUncertain = false) => api.post<BackgroundJob>(`/api/assistant/library-tags/model-batches/${encodeURIComponent(id)}/actions`, { cancel, remote_batch_id: remoteBatchId, abandon_uncertain: abandonUncertain }),
  getManualTagCatalog: () =>
    api.get<ManualTagCatalog>("/api/assistant/library-tags/catalog"),
  getTagVocabulary: () =>
    api.get<TagVocabulary>("/api/assistant/library-tags/vocabulary"),
  updateTagVocabulary: (
    expectedRevision: number,
    groups: TagVocabularyGroup[],
  ) =>
    api.put<TagVocabulary>("/api/assistant/library-tags/vocabulary", {
      schema_version: "assistant-tag-vocabulary/v1",
      expected_revision: expectedRevision,
      groups,
    }),
  listLibraryTags: (
    params: {
      model_status?: ModelTagFilter;
      model_job_id?: string;
      suggestion_source?: TagSuggestionSource;
      search?: string;
      tag?: string;
      review?: AnalysisTagReviewDecision;
      folder?: string;
      recursive?: boolean;
      offset?: number;
      limit?: number;
    } = {},
  ) => {
    const query = new URLSearchParams();
    if (params.model_status) query.set("model_status", params.model_status);
    if (params.model_job_id) query.set("model_job_id", params.model_job_id);
    if (params.suggestion_source) query.set("suggestion_source", params.suggestion_source);
    if (params.search) query.set("search", params.search);
    if (params.tag) query.set("tag", params.tag);
    if (params.review) query.set("review", params.review);
    if (params.folder !== undefined) query.set("folder", params.folder);
    if (params.recursive !== undefined) query.set("recursive", String(params.recursive));
    if (params.offset !== undefined) query.set("offset", String(params.offset));
    if (params.limit !== undefined) query.set("limit", String(params.limit));
    const suffix = query.size > 0 ? `?${query.toString()}` : "";
    return api.get<LibraryTagPage>(`/api/assistant/library-tags${suffix}`).then(requireCurrentTagPage);
  },
  queryModelLibraryTags: (
    scope: ModelTaggingScope,
    review: AnalysisTagReviewDecision = "pending",
    offset = 0,
    limit = 50,
  ) =>
    api.post<LibraryTagPage>("/api/assistant/library-tags/query", {
      scope,
      review,
      offset,
      limit,
    }).then(requireCurrentTagPage),
  patchManualTags: (trackId: number, add: string[], remove: string[]) =>
    api.patch<LibraryTagTrack>(
      `/api/assistant/library-tags/${encodeURIComponent(trackId)}`,
      { add, remove },
    ).then(requireCurrentTagTrack),
  patchManualTagsBulk: (trackIds: number[], add: string[], remove: string[]) =>
    api.post<BulkManualTagResult>("/api/assistant/library-tags/bulk", {
      track_ids: trackIds,
      add,
      remove,
    }),
  renameManualTag: (source: string, target: string) =>
    api.post<ManualTagRenameResult>(
      "/api/assistant/library-tags/catalog/rename",
      { source, target },
    ),
  reviewAnalysisTag: (
    trackId: number,
    suggestion: Pick<
      AnalysisTagSuggestion,
      "tag" | "analyzer_id" | "source_signature"
    >,
    decision: AnalysisTagReviewDecision,
  ) =>
    api.put<AnalysisTagReviewResult>(
      `/api/assistant/library-tags/${encodeURIComponent(trackId)}/analysis-tags/review`,
      {
        tag: suggestion.tag,
        analyzer_id: suggestion.analyzer_id,
        source_signature: suggestion.source_signature,
        decision,
      },
    ),
  reviewAnalysisTagsBulk: (
    items: AnalysisTagReviewTarget[],
    decision: BulkAnalysisTagReviewDecision,
  ) =>
    api.post<BulkAnalysisTagReviewResult>(
      "/api/assistant/library-tags/analysis-tags/reviews/bulk",
      { items, decision },
    ),
};
