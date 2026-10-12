import type * as Http from "@/generated/http";

import { api } from "./http";
import type { AnalysisTagReviewTarget } from "./assistant";
import type { BackgroundJob } from "./jobs";

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
