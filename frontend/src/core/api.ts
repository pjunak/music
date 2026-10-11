import { requireCurrentTagPage, requireCurrentTagTrack } from "./analysisResponse";

import type {
  AutomaticPlaylistApplyResult,
  AutomaticPlaylistPreview,
  AutomaticPlaylistRule,
  Cue,
  FolderEntry,
  FoldersResponse,
  InterruptSpec,
  KnownDevice,
  ModeDetail,
  ModeSummary,
  PlaylistMeta,
  PresetEffect,
  PresetManifest,
  SoundboardManifest,
  Track,
  TrackInPlaylist,
  TreeResponse,
} from "@/core/types";
import type * as Http from "@/generated/http";

const BASE = import.meta.env.VITE_API_BASE_URL ?? "";

export class ApiError extends Error {
  constructor(
    public status: number,
    public detail: string,
  ) {
    super(`API ${status}: ${detail}`);
  }
}

/** Called on every 401 response, before the ApiError is thrown to the
 *  caller. Registered by the auth store (a direct import here would be a
 *  cycle — auth already imports this module). The handler decides whether
 *  the 401 means "session lost mid-use" (operator was authenticated) or
 *  nothing (boot-time /me probe, wrong password on login). */
let onUnauthorized: (() => void) | null = null;

export function setUnauthorizedHandler(fn: () => void): void {
  onUnauthorized = fn;
}

async function request<T>(method: string, path: string, body?: unknown): Promise<T> {
  const init: RequestInit = { method, credentials: "include" };
  if (body instanceof FormData) {
    init.body = body;
  } else if (body !== undefined) {
    init.headers = { "Content-Type": "application/json" };
    init.body = JSON.stringify(body);
  }
  let response: Response;
  try {
    response = await fetch(`${BASE}${path}`, init);
  } catch (err) {
    // fetch() rejecting (vs an HTTP error) is a network-level failure — the
    // classic case is the first request after the device wakes from
    // minimize, while Wi-Fi/DNS are still coming back. GETs are idempotent,
    // so retry once after a beat instead of surfacing a warning for a blip
    // that self-heals a second later. Mutations still fail fast.
    if (method !== "GET") throw err;
    await new Promise((r) => setTimeout(r, 1000));
    response = await fetch(`${BASE}${path}`, init);
  }

  if (response.status === 204) {
    return undefined as T;
  }

  const text = await response.text();
  const contentType = response.headers.get("content-type") ?? "";
  if (!response.ok) {
    throw responseError(response.status, response.statusText, contentType, text);
  }
  if (!text) return undefined as T;
  if (!contentType.includes("application/json")) {
    throw responseError(response.status, response.statusText, contentType, text);
  }
  try {
    return JSON.parse(text) as T;
  } catch {
    // A success status with an unreadable body must not reach callers as undefined.
    throw new ApiError(response.status, "malformed JSON response");
  }
}

/** Build the ApiError for a failed or non-JSON response, notifying the auth
 *  store on 401. Shared by fetch, XHR uploads and file downloads. */
function responseError(
  status: number,
  statusText: string,
  contentType: string,
  text: string,
): ApiError {
  const looksLikeJson = contentType.includes("application/json");
  let fromJson: string | null = null;
  if (text && looksLikeJson) {
    try {
      const parsed: unknown = JSON.parse(text);
      if (parsed && typeof parsed === "object" && "detail" in parsed) {
        fromJson = formatApiDetail(parsed.detail);
      }
    } catch {
      // fall through to the status text.
    }
  }
  const snippet = text.slice(0, 200).replace(/\s+/g, " ").trim();
  const detail =
    fromJson ??
    (looksLikeJson || !text
      ? statusText || `HTTP ${status}`
      : `non-JSON response (${contentType || "no content-type"}): ${snippet}`);
  if (status === 401) onUnauthorized?.();
  return new ApiError(status, detail);
}

/** Download a binary API response (e.g. a backup archive) as a Blob. */
export async function downloadBlob(path: string): Promise<{ blob: Blob; filename: string | null }> {
  const response = await fetch(`${BASE}${path}`, { credentials: "include" });
  if (!response.ok) {
    const text = await response.text().catch(() => "");
    throw responseError(
      response.status,
      response.statusText,
      response.headers.get("content-type") ?? "",
      text,
    );
  }
  const disposition = response.headers.get("content-disposition") ?? "";
  const filename = disposition.match(/filename="([^"]+)"/)?.[1] ?? null;
  return { blob: await response.blob(), filename };
}

export const api = {
  get: <T>(path: string) => request<T>("GET", path),
  post: <T>(path: string, body?: unknown) => request<T>("POST", path, body),
  put: <T>(path: string, body?: unknown) => request<T>("PUT", path, body),
  patch: <T>(path: string, body?: unknown) => request<T>("PATCH", path, body),
  delete: <T>(path: string) => request<T>("DELETE", path),
};

// XHR (not fetch) because only XHR exposes upload-progress events.
function uploadWithProgress<T>(
  path: string,
  files: File[],
  onProgress?: (loaded: number, total: number) => void,
): Promise<T> {
  const form = new FormData();
  for (const f of files) form.append("files", f, f.name);
  return new Promise<T>((resolve, reject) => {
    const xhr = new XMLHttpRequest();
    xhr.open("POST", `${BASE}${path}`);
    xhr.withCredentials = true;
    xhr.responseType = "text";
    xhr.upload.onprogress = (e) => {
      if (onProgress && e.lengthComputable) onProgress(e.loaded, e.total);
    };
    xhr.onerror = () => reject(new Error("network error during upload"));
    xhr.onload = () => {
      if (xhr.status >= 200 && xhr.status < 300) {
        try {
          resolve(JSON.parse(xhr.responseText) as T);
        } catch {
          reject(new Error("upload succeeded but response wasn't JSON"));
        }
        return;
      }
      reject(
        responseError(
          xhr.status,
          xhr.statusText,
          xhr.getResponseHeader("content-type") ?? "",
          xhr.responseText,
        ),
      );
    };
    xhr.send(form);
  });
}

// --- typed helpers per resource -----------------------------------------

export const modesApi = {
  list: () => api.get<ModeSummary[]>("/api/modes"),
  get: (id: string) => api.get<ModeDetail>(`/api/modes/${encodeURIComponent(id)}`),
};

export type AuthoringImportKind =
  | "playlist"
  | "soundboard"
  | "interrupt"
  | "preset"
  | "cue";

export type AuthoringImportItem = Http.AuthoringImportItem;

export type AuthoringImportIssue = Http.AuthoringImportIssue;

function formatApiDetail(detail: unknown): string | null {
  if (typeof detail === "string") return detail;
  if (Array.isArray(detail)) {
    const messages = detail.flatMap((entry) => {
      if (!entry || typeof entry !== "object") return [];
      const candidate = entry as { loc?: unknown; msg?: unknown };
      if (typeof candidate.msg !== "string") return [];
      const location = Array.isArray(candidate.loc)
        ? candidate.loc.filter((part) => part !== "body").join(".")
        : "";
      return [`${location ? `${location}: ` : ""}${candidate.msg}`];
    });
    if (messages.length > 0) return messages.slice(0, 4).join(" · ");
  }
  if (detail && typeof detail === "object") {
    const providerDetail = detail as { message?: unknown };
    if (typeof providerDetail.message === "string") return providerDetail.message;
    try {
      return JSON.stringify(detail);
    } catch {
      return null;
    }
  }
  return typeof detail === "number" || typeof detail === "boolean" || typeof detail === "bigint"
    ? String(detail)
    : null;
}

export type AuthoringImportPreview = Http.AuthoringImportPreview;

export type AuthoringImportSelection = Http.AuthoringImportSelection;

export type AuthoringImportResult = Http.AuthoringImportResult;

export const authoringImportApi = {
  previewMode: (sourceModeId: string, targetModeId: string) =>
    api.post<AuthoringImportPreview>("/api/authoring/import/preview", {
      source_mode_id: sourceModeId,
      target_mode_id: targetModeId,
    }),
  commitMode: (
    sourceModeId: string,
    targetModeId: string,
    items: AuthoringImportSelection[],
  ) =>
    api.post<AuthoringImportResult>("/api/authoring/import/commit", {
      source_mode_id: sourceModeId,
      target_mode_id: targetModeId,
      items,
    }),
  previewDocument: (
    targetModeId: string,
    document: unknown,
    sourceName?: string,
  ) =>
    api.post<AuthoringImportPreview>("/api/authoring/import/document/preview", {
      target_mode_id: targetModeId,
      source_name: sourceName,
      document,
    }),
  commitDocument: (
    targetModeId: string,
    document: unknown,
    items: AuthoringImportSelection[],
    sourceName?: string,
  ) =>
    api.post<AuthoringImportResult>("/api/authoring/import/document/commit", {
      target_mode_id: targetModeId,
      source_name: sourceName,
      document,
      items,
    }),
};

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

export type BackgroundJobStatus =
  | "queued"
  | "running"
  | "cancel_requested"
  | "succeeded"
  | "failed"
  | "cancelled";

export type BackgroundJob = Http.BackgroundJobOut;

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

export const jobsApi = {
  list: (
    params: {
      kind?: string;
      status?: BackgroundJobStatus;
      limit?: number;
    } = {},
  ) => {
    const query = new URLSearchParams();
    if (params.kind !== undefined) query.set("kind", params.kind);
    if (params.status !== undefined) query.set("status", params.status);
    if (params.limit !== undefined) query.set("limit", String(params.limit));
    const suffix = query.size > 0 ? `?${query.toString()}` : "";
    return api.get<BackgroundJob[]>(`/api/jobs${suffix}`);
  },
  get: (jobId: string) =>
    api.get<BackgroundJob>(`/api/jobs/${encodeURIComponent(jobId)}`),
  cancel: (jobId: string) =>
    api.post<BackgroundJob>(`/api/jobs/${encodeURIComponent(jobId)}/cancel`),
  retry: (jobId: string) =>
    api.post<BackgroundJob>(`/api/jobs/${encodeURIComponent(jobId)}/retry`),
};

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

export type ActiveSession = Http.ActiveSessionResponse;

export const authApi = {
  listSessions: () => api.get<ActiveSession[]>("/api/auth/sessions"),
  revokeSession: (sessionId: string) =>
    api.delete<void>(
      `/api/auth/sessions/${encodeURIComponent(sessionId)}`,
    ),
};

export type { PresetManifest, PresetEffect } from "@/core/types";

export const presetsApi = {
  // Presets are per-mode now — list the given mode's EQ presets.
  list: (modeId: string) =>
    api.get<PresetManifest[]>(
      `/api/modes/${encodeURIComponent(modeId)}/presets`,
    ),
};

export const devicesApi = {
  list: () => api.get<KnownDevice[]>("/api/devices"),
  /** Remember a device (or update its name / output designation). */
  save: (clientId: string, payload: { name: string; is_output: boolean }) =>
    api.put<KnownDevice>(`/api/devices/${encodeURIComponent(clientId)}`, payload),
  remove: (clientId: string) =>
    api.delete<void>(`/api/devices/${encodeURIComponent(clientId)}`),
};

export const playlistsApi = {
  list: (params: { mode_id?: string; category?: string } = {}) => {
    const q = new URLSearchParams();
    if (params.mode_id !== undefined) q.set("mode_id", params.mode_id);
    if (params.category !== undefined) q.set("category", params.category);
    const qs = q.toString();
    return api.get<PlaylistMeta[]>(`/api/playlists${qs ? `?${qs}` : ""}`);
  },
  tracks: (playlistId: number) =>
    api.get<TrackInPlaylist[]>(`/api/playlists/${playlistId}/tracks`),
  create: (payload: { name: string; mode_id?: string | null; category?: string | null }) =>
    api.post<PlaylistMeta>("/api/playlists", payload),
  delete: (playlistId: number) => api.delete<void>(`/api/playlists/${playlistId}`),
  addTrack: (playlistId: number, trackId: number, position?: number) =>
    api.post<TrackInPlaylist>(`/api/playlists/${playlistId}/tracks`, {
      track_id: trackId,
      position,
    }),
  removeTrack: (playlistId: number, position: number) =>
    api.delete<void>(`/api/playlists/${playlistId}/tracks/${position}`),
  moveTrack: (playlistId: number, position: number, toPosition: number) =>
    api.patch<void>(`/api/playlists/${playlistId}/tracks/${position}`, {
      to_position: toPosition,
    }),
  update: (
    playlistId: number,
    payload: { name: string; mode_id?: string | null; category?: string | null },
  ) => api.patch<PlaylistMeta>(`/api/playlists/${playlistId}`, payload),
  previewAutomatic: (playlistId: number, rule: AutomaticPlaylistRule) =>
    api.post<AutomaticPlaylistPreview>(
      `/api/playlists/${playlistId}/automatic/preview`,
      { rule },
    ),
  configureAutomatic: (
    playlistId: number,
    rule: AutomaticPlaylistRule,
    sourceSignature: string,
  ) =>
    api.put<AutomaticPlaylistApplyResult>(
      `/api/playlists/${playlistId}/automatic`,
      { rule, source_signature: sourceSignature },
    ),
  refreshAutomatic: (playlistId: number) =>
    api.post<AutomaticPlaylistApplyResult>(
      `/api/playlists/${playlistId}/automatic/refresh`,
    ),
  disableAutomatic: (playlistId: number) =>
    api.delete<PlaylistMeta>(`/api/playlists/${playlistId}/automatic`),
  exportUrl: (playlistId: number, format: "m3u" | "json") =>
    `${BASE}/api/playlists/${playlistId}/export?format=${format}`,
};

export type LibrarySortKey =
  | "title"
  | "artist"
  | "album"
  | "album_artist"
  | "year"
  | "length_s"
  | "track_no"
  | "added_at"
  | "path";
export type SortOrder = "asc" | "desc";

export interface SearchParams {
  q?: string;
  limit?: number;
  offset?: number;
  sort?: LibrarySortKey;
  order?: SortOrder;
}

export type SearchResponse = Http.SearchResponse;

export type UploadResult = Http.UploadResult;

/** How an upload handles a filename that already exists at the destination.
 *  "rename" keeps both (foo.mp3 -> foo-1.mp3), "overwrite" replaces, "skip"
 *  leaves the existing file untouched. */
export type UploadConflict = "rename" | "overwrite" | "skip";

export type UploadCheckItem = Http.UploadCheckItem;

export type RescanResult = Http.RescanResult;

export type MetadataUpdate = Http.TrackMetadataUpdate;

export type BulkMetadataUpdate = Http.BulkMetadataUpdate;

export type BulkMetadataSkip = Http.BulkMetadataSkip;

export type BulkMetadataResult = Http.BulkMetadataResult;

export type BulkActionSkip = Http.BulkActionSkip;

export type BulkMoveResult = Http.BulkMoveResult;

export type BulkDeleteResult = Http.BulkDeleteResult;

export const libraryApi = {
  getTrack: (id: number) => api.get<Track>(`/api/library/tracks/${id}`),
  search: (params: SearchParams = {}) => {
    const qs = new URLSearchParams();
    if (params.q !== undefined) qs.set("q", params.q);
    if (params.limit !== undefined) qs.set("limit", String(params.limit));
    if (params.offset !== undefined) qs.set("offset", String(params.offset));
    if (params.sort !== undefined) qs.set("sort", params.sort);
    if (params.order !== undefined) qs.set("order", params.order);
    const query = qs.toString();
    return api.get<SearchResponse>(
      query ? `/api/library/search?${query}` : "/api/library/search",
    );
  },
  tree: (path = "") =>
    api.get<TreeResponse>(
      path ? `/api/library/tree?path=${encodeURIComponent(path)}` : "/api/library/tree",
    ),
  allFolders: () => api.get<FoldersResponse>("/api/library/folders"),
  upload: (
    files: File[],
    dest: string,
    onProgress?: (loaded: number, total: number) => void,
    conflict?: UploadConflict,
  ): Promise<UploadResult> =>
    uploadWithProgress<UploadResult>(
      `/api/library/upload?dest=${encodeURIComponent(dest)}${
        conflict ? `&conflict=${conflict}` : ""
      }`,
      files,
      onProgress,
    ),
  /** Preflight: which of these (dest, name) targets already exist on disk. */
  checkUpload: (items: UploadCheckItem[]) =>
    api.post<{ collisions: UploadCheckItem[] }>("/api/library/upload/check", {
      items,
    }),
  rescan: () => api.post<RescanResult>("/api/library/rescan"),
  updateBulkMetadata: (payload: BulkMetadataUpdate) =>
    api.patch<BulkMetadataResult>("/api/library/tracks/bulk-metadata", payload),
  moveTrack: (id: number, destination: string, newFilename?: string) =>
    api.post<Track>(`/api/library/tracks/${id}/move`, {
      destination,
      new_filename: newFilename,
    }),
  deleteTrack: (id: number) => api.delete<void>(`/api/library/tracks/${id}`),
  bulkMove: (trackIds: number[], destination: string) =>
    api.post<BulkMoveResult>("/api/library/tracks/bulk-move", {
      track_ids: trackIds,
      destination,
    }),
  bulkDelete: (trackIds: number[]) =>
    api.post<BulkDeleteResult>("/api/library/tracks/bulk-delete", {
      track_ids: trackIds,
    }),
  coverUrl: (id: number) => `${BASE}/api/library/tracks/${id}/cover`,
  streamUrl: (id: number) => `${BASE}/api/library/tracks/${id}/stream`,
  createFolder: (path: string) =>
    api.post<{ name: string; path: string; track_count: number; has_children: boolean }>(
      "/api/library/folders",
      { path },
    ),
  deleteFolder: (path: string, recursive: boolean) => {
    const qs = new URLSearchParams({ path, recursive: String(recursive) });
    return api.delete<{ removed_tracks: number }>(
      `/api/library/folders?${qs.toString()}`,
    );
  },
  renameFolder: (src: string, dst: string) =>
    api.post<{ name: string; path: string; track_count: number; has_children: boolean }>(
      "/api/library/folders/rename",
      { src, dst },
    ),
};

// --- library cleanup -----------------------------------------------------

// Keep in lockstep with the Rust cleanup rules and generated API contract.
export type CleanupRuleId =
  | "strip_track_numbers"
  | "strip_artist"
  | "strip_album"
  | "strip_junk"
  | "normalize_separators"
  | "normalize_case"
  | "tag_title"
  | "tag_artist"
  | "tag_album"
  | "tag_number"
  | "tag_year"
  | "rename_folders";

export type CleanupScope = Http.CleanupScope;

export interface CleanupOp {
  op_id: string;
  track_id: number;
  kind: "rename" | "tag";
  field: string | null;
  old: string | number | null;
  new: string | number | null;
  rules: string[];
  confidence: "high" | "low";
  /** Value confirmed by an online name lookup (MusicBrainz). */
  verified: boolean;
  evidence?: { source: string; entity: string; id?: string; recording_id?: string; release_id?: string | null; method?: string };
  /** Stable evidence identity used by the rejection pool; excludes job IDs and retrieval time. */
  review_context?: string;
}

export interface CleanupReviewProposal extends Omit<CleanupOp, "kind" | "evidence"> {
  kind: "tag" | "rename" | "folder_rename";
  path: string;
  evidence: CleanupOp["evidence"] | null;
  evidence_context: string | null;
}
// The server stores proposals as opaque review records; the frontend reads
// them back with the same evidence shape it submitted.
export type CleanupRejectedItem = Omit<Http.CleanupRejectedItem, "proposal"> & {
  proposal: CleanupReviewProposal;
};
export type CleanupRejectedPage = Omit<Http.CleanupRejectedPage, "items"> & {
  items: CleanupRejectedItem[];
};

export type CleanupTrackPlan = Http.CleanupTrackPlanOut;

/** A proposed folder rename (leaf-only — `path` stays, its last segment
 *  becomes `new`). Reviewed and applied alongside the per-track ops. */
export type CleanupFolderSuggestion = Http.CleanupFolderOut;

export type CleanupAnalyzeResult = Http.AnalyzeResult;

export type CleanupVerifyResult = Http.VerifyResult;

export type CleanupOpIn = Http.CleanupOpIn;

export type CleanupApplyResult = Http.ApplyResult;

export type CleanupBatchSummary = Http.BatchSummary;

export interface CleanupBatchDetail extends CleanupBatchSummary {
  items: unknown[];
}

export type CleanupRevertResult = Http.RevertResult;

export type CleanupSource = Http.CleanupSourceOut;

export interface CleanupCatalogTagSuggestion extends AnalysisTagReviewTarget {
  source_tag: string;
  count: number;
  confidence: "medium";
}

export interface CleanupImportedEvidence {
  track_id: number;
  fields: Record<string, string>;
  source?: string;
  propose?: boolean;
}

export interface CleanupEditionReview {
  release_id: string; compared_release_ids: string[]; title: string; description: string; formats: string[]; labels: string[];
  folder_tracks: number; compared_tracks: number; release_tracks: number;
  title_matches: number; duration_matches: number; duration_conflicts: number;
  distinguishing_tracks: { title: string; disc_no: number | null; track_no: number | null; present: boolean; duration_agrees: boolean }[];
  distinguishing_tracks_total: number; missing_titles: string[]; missing_titles_total: number;
  complete: boolean; recommended: boolean;
}

export interface CleanupReleaseChoice {
  edition_review?: CleanupEditionReview;
  id: string;
  title: string;
  artist: string;
  date: string | null;
  country: string | null;
  barcode: string | null;
  catalog_numbers: string[];
  assignment: { classification: string; considered: number; matched: number; unmatched_tracks: number[]; unmatched_slots: string[] };
  ops: CleanupOp[];
}

export interface CleanupEnrichmentPlan {
  schema: "library-cleanup-enrichment/v1";
  partial?: boolean;
  track_id: number;
  path: string;
  status: "identified" | "fingerprinted" | "unmatched" | "failed";
  identity: {
    recording_mbid: string;
    method: "metadata" | "fingerprint" | "identifier" | "isrc" | "local_hypothesis";
    confidence: number;
    title: string;
    artist: string;
    release_mbid: string | null;
  } | null;
  ops: CleanupOp[];
  tag_suggestions: CleanupCatalogTagSuggestion[];
  notes: string[];
  error_code?: string;
  retrieved_at?: number;
  local_evidence_signature?: string;
  folder_context_signature?: string;
  evidence_revision?: number;
  local_evidence?: { observations: { field: string; value: string; source: string }[]; notes: string[] };
  recording_observations?: { first_release_date: string | null; genres: string[]; credits: string[] };
  release_choices?: CleanupReleaseChoice[];
  candidates?: { id: string; title: string; artist: string; length_ms: number | null; provider_score: number }[];
}

export interface CleanupEnrichmentResult {
  schema: "library-cleanup-enrichment/v1";
  scanned: number;
  identified: number;
  fingerprinted: number;
  unmatched: number;
  failed: number;
  cached: number;
  plans: CleanupEnrichmentPlan[];
}

export type CleanupModelStatus = Http.CleanupModelStatus;

export interface CleanupModelResult {
  schema_version: "assistant-library-cleanup-result/v1";
  mode?: "recording" | "edition";
  recommended_release_id?: string | null;
  track_id: number;
  decision: { decision: string; reason: string; candidate_id: string | null; evidence_ids: string[] };
  ops: CleanupOp[];
  source_signature?: string;
  role_fingerprint?: string;
}

export const cleanupApi = {
  rejected: (before: number | null = null, search = "") => {
    const query = new URLSearchParams({ search });
    if (before !== null) query.set("before", String(before));
    return api.get<CleanupRejectedPage>(`/api/library/cleanup/rejections?${query}`);
  },
  matchRejected: (proposals: CleanupReviewProposal[]) => api.post<(number | null)[]>("/api/library/cleanup/rejections/match", proposals),
  reject: (proposal: CleanupReviewProposal) => api.post<CleanupRejectedItem>("/api/library/cleanup/rejections", proposal),
  restoreRejected: (id: number) => api.post<CleanupReviewProposal>(`/api/library/cleanup/rejections/${id}/restore`),
  forgetRejected: (id: number) => api.delete<void>(`/api/library/cleanup/rejections/${id}`),
  modelStatus: () => api.get<CleanupModelStatus>("/api/library/cleanup/model"),
  reviewCandidates: (trackId: number, catalogJobId: string, disclosureVersion: string, editionReview = false) =>
    api.post<BackgroundJob>("/api/library/cleanup/model/jobs", {
      track_id: trackId, catalog_job_id: catalogJobId, disclosure_version: disclosureVersion, consent: true, edition_review: editionReview,
    }),
  analyze: (scope: CleanupScope, rules: CleanupRuleId[]) =>
    api.post<CleanupAnalyzeResult>("/api/library/cleanup/analyze", { scope, rules }),
  /** Resolve a small batch of names against MusicBrainz (server paces at
   *  1 req/s — keep batches ≤ 5 and chunk longer lists). */
  verify: (names: string[]) =>
    api.post<CleanupVerifyResult>("/api/library/cleanup/verify", { names }),
  sources: () => api.get<CleanupSource[]>("/api/library/cleanup/sources"),
  updateSource: (sourceId: string, enabled: boolean) =>
    api.put<CleanupSource>(`/api/library/cleanup/sources/${encodeURIComponent(sourceId)}`, {
      enabled,
    }),
  saveSourceCredential: (sourceId: string, apiKey: string) =>
    api.put<CleanupSource>(
      `/api/library/cleanup/sources/${encodeURIComponent(sourceId)}/credential`,
      { api_key: apiKey },
    ),
  deleteSourceCredential: (sourceId: string) =>
    api.delete<CleanupSource>(
      `/api/library/cleanup/sources/${encodeURIComponent(sourceId)}/credential`,
    ),
  enrich: (scope: CleanupScope, force = false, imports: CleanupImportedEvidence[] = []) =>
    api.post<BackgroundJob>("/api/library/cleanup/enrichment-jobs", {
      scope,
      force,
      imports,
    }),
  /** One chunk of accepted ops. Pass the batch_id from the previous chunk
   *  so the whole run lands in a single revertable journal. */
  apply: (ops: CleanupOpIn[], batchId: number | null, scopeLabel: string) =>
    api.post<CleanupApplyResult>("/api/library/cleanup/apply", {
      ops,
      batch_id: batchId,
      scope_label: scopeLabel,
    }),
  batches: () => api.get<CleanupBatchSummary[]>("/api/library/cleanup/batches"),
  batch: (id: number) =>
    api.get<CleanupBatchDetail>(`/api/library/cleanup/batches/${id}`),
  revertBatch: (id: number) =>
    api.post<CleanupRevertResult>(`/api/library/cleanup/batches/${id}/revert`),
  /** Revert from a previously-downloaded journal file (disaster path —
   *  works even after the server-side batch rows are gone). */
  revertJournal: (items: unknown[]) =>
    api.post<CleanupRevertResult>("/api/library/cleanup/revert", { items }),
};

// --- SFX ---------------------------------------------------------------

export type SfxFile = Http.SfxFileOut;

export type SfxFolder = Http.SfxFolderOut;

// Files immediately in one folder; the hierarchy comes from /api/sfx/folders.
export type SfxTreeResponse = Http.SfxTreeResponse;

export type SfxFoldersResponse = Http.SfxFoldersResponse;

export type SfxUploadResult = Http.SfxUploadResult;

export const sfxApi = {
  allFiles: () => api.get<SfxFile[]>("/api/sfx/files"),
  tree: (path = "") =>
    api.get<SfxTreeResponse>(
      path ? `/api/sfx/tree?path=${encodeURIComponent(path)}` : "/api/sfx/tree",
    ),
  allFolders: () => api.get<SfxFoldersResponse>("/api/sfx/folders"),
  upload: (
    files: File[],
    dest: string,
    onProgress?: (loaded: number, total: number) => void,
    conflict?: UploadConflict,
  ): Promise<SfxUploadResult> =>
    uploadWithProgress<SfxUploadResult>(
      `/api/sfx/upload?dest=${encodeURIComponent(dest)}${
        conflict ? `&conflict=${conflict}` : ""
      }`,
      files,
      onProgress,
    ),
  checkUpload: (items: UploadCheckItem[]) =>
    api.post<{ collisions: UploadCheckItem[] }>("/api/sfx/upload/check", { items }),
  moveFile: (src: string, dstFolder: string, newFilename?: string) =>
    api.post<SfxFile>("/api/sfx/move", {
      src,
      dst_folder: dstFolder,
      new_filename: newFilename,
    }),
  deleteFile: (path: string) =>
    api.delete<void>(`/api/sfx/files?path=${encodeURIComponent(path)}`),
  createFolder: (path: string) =>
    api.post<SfxFolder>("/api/sfx/folders", { path }),
  deleteFolder: (path: string, recursive: boolean) => {
    const qs = new URLSearchParams({ path, recursive: String(recursive) });
    return api.delete<{ deleted: boolean }>(`/api/sfx/folders?${qs.toString()}`);
  },
  renameFolder: (src: string, dst: string) =>
    api.post<SfxFolder>("/api/sfx/folders/rename", { src, dst }),
  fileUrl: (path: string) =>
    `${BASE}/api/sfx/file?path=${encodeURIComponent(path)}`,
};

// --- modes scaffolding -------------------------------------------------

export const modesAdminApi = {
  create: (id: string, name: string) =>
    api.post<ModeSummary>("/api/modes", { id, name }),
  rename: (id: string, name: string) =>
    api.patch<ModeSummary>(`/api/modes/${encodeURIComponent(id)}`, { name }),
  delete: (id: string) =>
    api.delete<void>(`/api/modes/${encodeURIComponent(id)}`),
  reload: () =>
    api.post<{ loaded: string[]; errors: Record<string, string> }>(
      "/api/modes/reload",
    ),
  createSoundboard: (modeId: string, payload: { id: string; name?: string }) =>
    api.post<unknown>(
      `/api/modes/${encodeURIComponent(modeId)}/soundboards`,
      payload,
    ),
  deleteSoundboard: (modeId: string, soundboardId: string) =>
    api.delete<void>(
      `/api/modes/${encodeURIComponent(modeId)}/soundboards/${encodeURIComponent(soundboardId)}`,
    ),
  // Soundboard editor — categories + items inside an existing soundboard.
  // Each call returns the updated SoundboardManifest so the UI can re-render
  // without a separate fetch.
  addCategory: (
    modeId: string,
    soundboardId: string,
    payload: { id: string; name: string },
  ) =>
    api.post<SoundboardManifest>(
      `/api/modes/${encodeURIComponent(modeId)}/soundboards/${encodeURIComponent(soundboardId)}/categories`,
      payload,
    ),
  deleteCategory: (modeId: string, soundboardId: string, categoryId: string) =>
    api.delete<SoundboardManifest>(
      `/api/modes/${encodeURIComponent(modeId)}/soundboards/${encodeURIComponent(soundboardId)}/categories/${encodeURIComponent(categoryId)}`,
    ),
  addItem: (
    modeId: string,
    soundboardId: string,
    categoryId: string,
    payload: { file: string; name: string; hotkey?: string; icon?: string },
  ) =>
    api.post<SoundboardManifest>(
      `/api/modes/${encodeURIComponent(modeId)}/soundboards/${encodeURIComponent(soundboardId)}/categories/${encodeURIComponent(categoryId)}/items`,
      payload,
    ),
  updateItem: (
    modeId: string,
    soundboardId: string,
    categoryId: string,
    index: number,
    payload: { name?: string; hotkey?: string; icon?: string; file?: string },
  ) =>
    api.patch<SoundboardManifest>(
      `/api/modes/${encodeURIComponent(modeId)}/soundboards/${encodeURIComponent(soundboardId)}/categories/${encodeURIComponent(categoryId)}/items/${index}`,
      payload,
    ),
  deleteItem: (
    modeId: string,
    soundboardId: string,
    categoryId: string,
    index: number,
  ) =>
    api.delete<SoundboardManifest>(
      `/api/modes/${encodeURIComponent(modeId)}/soundboards/${encodeURIComponent(soundboardId)}/categories/${encodeURIComponent(categoryId)}/items/${index}`,
    ),

  // Interrupt templates — saved on the mode's manifest.yaml.
  addInterrupt: (
    modeId: string,
    payload: {
      name: string;
      playlist?: string;
      soundboard_item?: string;
      fade_in_ms?: number;
      fade_out_ms?: number;
      return_to_ambient?: boolean;
      duck_to?: number | null;
    },
  ) =>
    api.post<InterruptSpec[]>(
      `/api/modes/${encodeURIComponent(modeId)}/interrupts`,
      payload,
    ),
  updateInterrupt: (
    modeId: string,
    index: number,
    payload: Partial<{
      name: string;
      playlist: string | null;
      soundboard_item: string | null;
      fade_in_ms: number;
      fade_out_ms: number;
      return_to_ambient: boolean;
      duck_to: number | null;
    }>,
  ) =>
    api.patch<InterruptSpec[]>(
      `/api/modes/${encodeURIComponent(modeId)}/interrupts/${index}`,
      payload,
    ),
  deleteInterrupt: (modeId: string, index: number) =>
    api.delete<InterruptSpec[]>(
      `/api/modes/${encodeURIComponent(modeId)}/interrupts/${index}`,
    ),

  createCue: (modeId: string, payload: Http.CreateCueRequest) =>
    api.post<Cue>(`/api/modes/${encodeURIComponent(modeId)}/cues`, payload),
  updateCue: (modeId: string, cueId: string, payload: Http.CueBody) =>
    api.put<Cue>(
      `/api/modes/${encodeURIComponent(modeId)}/cues/${encodeURIComponent(cueId)}`,
      payload,
    ),
  deleteCue: (modeId: string, cueId: string) =>
    api.delete<void>(
      `/api/modes/${encodeURIComponent(modeId)}/cues/${encodeURIComponent(cueId)}`,
    ),
};

// --- presets scaffolding -----------------------------------------------

// EQ presets are per-mode now — CRUD lives under the modes API.
export const presetsAdminApi = {
  create: (
    modeId: string,
    payload: {
      id: string;
      name: string;
      description?: string;
      effects: PresetEffect[];
      crossfade_ms?: number | null;
    },
  ) =>
    api.post<PresetManifest>(
      `/api/modes/${encodeURIComponent(modeId)}/presets`,
      payload,
    ),
  update: (
    modeId: string,
    presetId: string,
    payload: {
      name: string;
      description?: string;
      effects?: PresetEffect[];
      crossfade_ms?: number | null;
    },
  ) =>
    api.put<PresetManifest>(
      `/api/modes/${encodeURIComponent(modeId)}/presets/${encodeURIComponent(presetId)}`,
      payload,
    ),
  delete: (modeId: string, presetId: string) =>
    api.delete<void>(
      `/api/modes/${encodeURIComponent(modeId)}/presets/${encodeURIComponent(presetId)}`,
    ),
};

// Diagnostics — server-side operational snapshot. Read by the
// Diagnostics tab so the operator can see what's happening (track
// count, last rescan, mode load errors — a per-mode preset error is
// folded into that mode's error string — connected devices) without
// SSH'ing into the host.
export type LoaderStatus = Http.LoaderStatus;

export type DiagnosticsResponse = Http.DiagnosticsResponse;

export const diagnosticsApi = {
  get: () => api.get<DiagnosticsResponse>("/api/diagnostics"),
};

// Re-export types we already had so callers don't need to dig in /core/types.
export type { FolderEntry };
