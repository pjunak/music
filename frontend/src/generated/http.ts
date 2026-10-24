// Generated from the Rust OpenAPI document by `music-cli contracts export`.
// Do not edit by hand.

export type ActiveMode = {
  mode_id: string | null;
};

export type ActiveSessionResponse = {
  created_at: string;
  expires_at: string;
  is_current: boolean;
  last_seen: string;
  session_id: string;
};

export type AddCategoryRequest = {
  id: string;
  name: string;
};

export type AddItemRequest = {
  file: string;
  hotkey?: string | null;
  icon?: string | null;
  name: string;
};

export type AmbientState = {
  current_track_id: number | null;
  history?: number[];
  loop?: "off" | "follow" | "queue" | "track";
  position_anchored_at: number | null;
  position_ms?: number;
  queue?: number[];
  shuffle?: "off" | "random";
  source_playlist_id: number | null;
};

export type AnalysisTagReviewRequest = {
  analyzer_id: string;
  decision: "pending" | "accepted" | "rejected";
  source_signature: string;
  tag: string;
};

export type AnalysisTagReviewResult = {
  analyzer_id: string;
  decision: "pending" | "accepted" | "rejected";
  manual_tags: string[];
  source_signature: string;
  tag: string;
  track_id: number;
};

export type AnalysisTagSuggestionOut = {
  analyzer_id: string;
  contradiction_ids: string[];
  evidence: string[];
  evidence_ids: string[];
  source_signature: string;
  status: "pending" | "accepted" | "rejected";
  support: "supported" | "tentative";
  tag: string;
};

export type AnalyzeRequest = {
  /** Enabled rule ids; omit for the default set */
  rules?: (("strip_track_numbers" | "strip_artist" | "strip_album" | "strip_junk" | "normalize_separators" | "normalize_case" | "tag_title" | "tag_artist" | "tag_album" | "tag_number" | "tag_year" | "rename_folders")[]) | null;
  scope: CleanupScope;
};

export type AnalyzeResult = {
  folders: CleanupFolderOut[];
  pending_lookups: string[];
  plans: CleanupTrackPlanOut[];
  scanned: number;
};

export type ApplyRequest = {
  batch_id?: number | null;
  ops: CleanupOpIn[];
  scope_label?: string;
};

export type ApplyResult = {
  applied: number;
  batch_id: number | null;
  skipped: CleanupSkip[];
};

export type AuthoringDocumentCommitRequest = {
  document: AuthoringImportDocumentV1;
  items: AuthoringImportSelection[];
  source_name?: string | null;
  target_mode_id: string;
};

export type AuthoringDocumentPreviewRequest = {
  document: AuthoringImportDocumentV1;
  source_name?: string | null;
  target_mode_id: string;
};

export type AuthoringImportCommitRequest = {
  items: AuthoringImportSelection[];
  source_mode_id: string;
  target_mode_id: string;
};

export type AuthoringImportDocumentV1 = {
  cues?: ImportCue[];
  interrupts?: ImportInterrupt[];
  name?: string | null;
  playlists?: ImportPlaylist[];
  presets?: ImportPreset[];
  schema: "authoring-import/v1";
  soundboards?: ImportSoundboard[];
};

export type AuthoringImportIssue = {
  code: string;
  message: string;
  related_item: AuthoringImportSelection | null;
  severity: ImportIssueSeverity;
};

export type AuthoringImportItem = {
  issues: AuthoringImportIssue[];
  kind: AuthoringResourceKind;
  name: string;
  reason: string | null;
  resource_id: string;
  status: ImportItemStatus;
  summary: string;
};

export type AuthoringImportMode = {
  id: string;
  name: string;
};

export type AuthoringImportPreview = {
  items: AuthoringImportItem[];
  source: AuthoringImportSource;
  source_mode: AuthoringImportMode | null;
  target_mode: AuthoringImportMode;
};

export type AuthoringImportPreviewRequest = {
  source_mode_id: string;
  target_mode_id: string;
};

export type AuthoringImportResult = {
  imported: AuthoringImportItem[];
  missing_track_paths: string[];
  skipped: AuthoringImportItem[];
};

export type AuthoringImportSelection = {
  kind: AuthoringResourceKind;
  resource_id: string;
};

export type AuthoringImportSource = {
  id: string;
  name: string;
  type: AuthoringSourceType;
};

export type AuthoringResourceKind = "playlist" | "soundboard" | "interrupt" | "preset" | "cue";

export type AuthoringSourceType = "mode" | "document";

export type AutomaticPlaylistApplyResult = {
  materialized_tracks: number;
  playlist: PlaylistMeta;
  schema_version: "automatic-playlist-apply/v1";
};

export type AutomaticPlaylistConfigureRequest = {
  rule: AutomaticPlaylistRuleV1;
  source_signature: string;
};

export type AutomaticPlaylistPreview = {
  library_tracks: number;
  matched_tracks: number;
  schema_version: "automatic-playlist-preview/v1";
  source_signature: string;
  tracks: AutomaticTrackSummary[];
};

export type AutomaticPlaylistPreviewRequest = {
  rule: AutomaticPlaylistRuleV1;
};

export type AutomaticPlaylistRuleV1 = {
  exclude_tags: string[];
  include_tags: string[];
  include_unknown_bpm: boolean;
  match: "any" | "all";
  max_bpm: number | null;
  maximum_tracks: number;
  min_bpm: number | null;
  order_by: "title" | "newest" | "bpm_ascending" | "bpm_descending";
  schema: "automatic-playlist/v1";
  tag_sources: "manual";
};

export type AutomaticTrackSummary = {
  album: string;
  artist: string;
  bpm: number | null;
  id: number;
  length_s: number;
  path: string;
  title: string;
};

export type BackgroundJobOut = {
  attempts: number;
  created_at: string;
  error: string | null;
  finished_at: string | null;
  id: string;
  kind: string;
  parameters: Record<string, unknown>;
  progress_current: number;
  progress_message: string;
  progress_phase: string;
  progress_total: number | null;
  result: Record<string, unknown> | null;
  retry_of_id: string | null;
  started_at: string | null;
  status: "queued" | "running" | "cancel_requested" | "succeeded" | "failed" | "cancelled";
  updated_at: string;
};

export type BatchDetail = {
  created_at: string;
  id: number;
  item_count: number;
  items: Record<string, unknown>[];
  reverted_at: string | null;
  scope_label: string;
};

export type BatchSummary = {
  created_at: string;
  id: number;
  item_count: number;
  reverted_at: string | null;
  scope_label: string;
};

export type BulkActionSkip = {
  reason: string;
  track_id: number;
};

export type BulkAnalysisTagReviewApplied = {
  analyzer_id: string;
  decision: "accepted" | "rejected";
  source_signature: string;
  tag: string;
  track_id: number;
};

export type BulkAnalysisTagReviewFailure = {
  analyzer_id: string;
  code: "not_found" | "stale" | "tag_limit";
  error: string;
  source_signature: string;
  tag: string;
  track_id: number;
};

export type BulkAnalysisTagReviewItem = {
  analyzer_id: string;
  source_signature: string;
  tag: string;
  track_id: number;
};

export type BulkAnalysisTagReviewRequest = {
  decision: "accepted" | "rejected";
  items: BulkAnalysisTagReviewItem[];
};

export type BulkAnalysisTagReviewResult = {
  applied: BulkAnalysisTagReviewApplied[];
  failures: BulkAnalysisTagReviewFailure[];
  requested_items: number;
};

export type BulkDeleteRequest = {
  track_ids: number[];
};

export type BulkDeleteResult = {
  deleted_ids: number[];
  skipped: BulkActionSkip[];
};

export type BulkManualTagFailure = {
  error: string;
  track_id: number;
};

export type BulkManualTagPatch = {
  add?: string[];
  remove?: string[];
  track_ids: number[];
};

export type BulkManualTagResult = {
  changed_track_ids: number[];
  failures: BulkManualTagFailure[];
  matched_tracks: number;
  missing_track_ids: number[];
  requested_tracks: number;
};

export type BulkMetadataResult = {
  skipped: BulkMetadataSkip[];
  updated: TrackOut[];
};

export type BulkMetadataSkip = {
  reason: string;
  track_id: number;
};

export type BulkMetadataUpdate = {
  track_ids: number[];
  updates: TrackMetadataUpdate;
};

export type BulkMoveRequest = {
  destination: string;
  track_ids: number[];
};

export type BulkMoveResult = {
  moved: TrackOut[];
  skipped: BulkActionSkip[];
};

export type BulkRestoreRequest = {
  track_ids: number[];
};

export type BulkRestoreResult = {
  restored: TrackOut[];
  skipped: BulkActionSkip[];
};

export type CleanupEnrichmentRequest = {
  force?: boolean;
  imports?: unknown[];
  scope: CleanupScope;
};

export type CleanupFolderOut = {
  confidence: "high" | "low";
  new: string;
  old: string;
  op_id: string;
  path: string;
  rules: string[];
};

export type CleanupModelStart = {
  catalog_job_id: string;
  consent: boolean;
  disclosure_version: string;
  edition_review?: boolean;
  track_id: number;
};

export type CleanupModelStatus = {
  available: boolean;
  disclosure_version: string;
  maximum_candidates: number;
  may_incur_cost: boolean;
  model_id: string | null;
  never_shared: string[];
  reason_code: string | null;
  shared_with_provider: string[];
};

export type CleanupOpIn = {
  field?: string | null;
  kind: "rename" | "tag" | "folder_rename";
  new?: number | string | null;
  old?: number | string | null;
  path?: string;
  track_id: number;
};

export type CleanupOpOut = {
  confidence: "high" | "low";
  field: string | null;
  kind: "rename" | "tag";
  new: number | string | null;
  old: number | string | null;
  op_id: string;
  rules: string[];
  track_id: number;
  verified: boolean;
};

export type CleanupRejectedItem = {
  current: boolean;
  id: number;
  proposal: CleanupReviewProposal;
  rejected_at: number;
};

export type CleanupRejectedPage = {
  items: CleanupRejectedItem[];
  next_before: number | null;
};

export type CleanupReviewProposal = {
  confidence: string;
  evidence?: unknown;
  evidence_context: string | null;
  field: string | null;
  kind: string;
  new: number | string | null;
  old: number | string | null;
  op_id: string;
  path: string;
  rules: string[];
  track_id: number;
  verified: boolean;
};

export type CleanupScope = {
  path?: string;
  recursive?: boolean;
  track_ids?: number[];
  type: "all" | "folder" | "tracks";
};

export type CleanupSkip = {
  reason: string;
  track_id: number;
};

export type CleanupSourceCredentialUpdate = {
  api_key: string;
};

export type CleanupSourceOut = {
  available: boolean;
  capabilities: string[];
  configuration_hint: string | null;
  configured: boolean;
  credential_kind: string | null;
  credential_saved: boolean;
  credential_source: string | null;
  description: string;
  enabled: boolean;
  id: string;
  key_hint: string | null;
  label: string;
  unavailable_reason: string | null;
};

export type CleanupSourceUpdate = {
  enabled: boolean;
};

export type CleanupTrackPlanOut = {
  notes: string[];
  ops: CleanupOpOut[];
  path: string;
  track_id: number;
};

export type ComponentStatus = "starting" | "ready" | "degraded" | "failed";

export type CreateCueRequest = {
  description?: string | null;
  id: string;
  loops?: CueLoopIn[];
  name: string;
  playlist?: string | null;
  preset?: string | null;
  sfx?: CueSfxIn[];
  start_index?: number;
  start_ms?: number;
};

export type CreateModeRequest = {
  id: string;
  name: string;
};

export type CreatePresetRequest = {
  crossfade_ms?: number | null;
  description?: string | null;
  effects?: EffectSpecIn[];
  id: string;
  name: string;
};

export type CreateSoundboardRequest = {
  id: string;
  name?: string | null;
};

export type CueBody = {
  description?: string | null;
  loops?: CueLoopIn[];
  name: string;
  playlist?: string | null;
  preset?: string | null;
  sfx?: CueSfxIn[];
  start_index?: number;
  start_ms?: number;
};

export type CueLoop = {
  interval_s: number;
  item: string;
  soundboard: string;
  volume: number;
};

export type CueLoopIn = {
  interval_s: number;
  item: string;
  soundboard: string;
  volume?: number;
};

export type CueSfx = {
  item: string;
  soundboard: string;
  volume: number;
};

export type CueSfxIn = {
  item: string;
  soundboard: string;
  volume?: number;
};

export type CueSpec = {
  description: string | null;
  id: string;
  loops: CueLoop[];
  name: string;
  playlist: string | null;
  preset: string | null;
  sfx: CueSfx[];
  start_index: number;
  start_ms: number;
};

export type DeviceInfo = {
  client_id: string;
  device_id: string;
  is_output?: boolean;
  name: string;
};

export type DeviceOut = {
  added_at: string | null;
  client_id: string;
  connected: boolean;
  is_output: boolean;
  name: string;
};

export type DevicePut = {
  is_output?: boolean;
  name: string;
};

export type DiagnosticsResponse = {
  connected_device_count: number;
  last_scan_at: number | null;
  modes: LoaderStatus;
  state_revision: number;
  track_count: number;
};

export type DocumentSchemaContract = Record<string, unknown>;

export type EffectSpec = {
  type: string;
  [key: string]: unknown;
};

export type EffectSpecIn = {
  type: string;
  [key: string]: unknown;
};

export type EqDraftRequest = {
  goal: string;
  name: string;
};

export type ExportFileResponse = unknown;

export type FolderCreateRequest = {
  path: string;
};

export type FolderDeleteResult = {
  removed_tracks: number;
};

export type FolderOut = {
  has_children: boolean;
  name: string;
  path: string;
  track_count: number;
};

export type FolderRenameRequest = {
  dst: string;
  src: string;
};

export type FoldersResponse = {
  folders: FolderOut[];
};

/** FastAPI-compatible validation envelope retained as a stable HTTP contract. */
export type HTTPValidationError = {
  detail?: ValidationError[];
};

export type ImportCue = {
  description?: string | null;
  id: string;
  loops?: ImportCueLoop[];
  name: string;
  playlist?: string | null;
  preset?: string | null;
  sfx?: ImportCueSfx[];
  start_index?: number;
  start_ms?: number;
};

export type ImportCueLoop = {
  interval_s: number;
  item: string;
  soundboard: string;
  volume?: number;
};

export type ImportCueSfx = {
  item: string;
  soundboard: string;
  volume?: number;
};

export type ImportEffect = {
  type: string;
  [key: string]: unknown;
};

export type ImportInterrupt = {
  duck_to?: number | null;
  fade_in_ms?: number;
  fade_out_ms?: number;
  name: string;
  playlist?: string | null;
  return_to_ambient?: boolean;
  soundboard_item?: string | null;
};

export type ImportIssueSeverity = "warning" | "error";

export type ImportItemStatus = "ready" | "conflict" | "invalid";

export type ImportPlaylist = {
  category?: string | null;
  name: string;
  tracks?: string[];
};

export type ImportPreset = {
  crossfade_ms?: number | null;
  description?: string | null;
  effects?: ImportEffect[];
  id: string;
  name: string;
};

export type ImportSoundboard = {
  categories?: ImportSoundboardCategory[];
  id: string;
  name?: string | null;
};

export type ImportSoundboardCategory = {
  id: string;
  items?: ImportSoundboardItem[];
  name: string;
};

export type ImportSoundboardItem = {
  file: string;
  hotkey?: string | null;
  icon?: string | null;
  name: string;
};

export type IntegrationsSpec = {
  lights: Record<string, unknown> | null;
};

export type InterruptSpec = {
  duck_to: number | null;
  fade_in_ms: number;
  fade_out_ms: number;
  name: string;
  playlist: string | null;
  return_to_ambient: boolean;
  soundboard_item: string | null;
};

export type InterruptState = {
  current_track_id: number;
  duck_to: number | null;
  fade_in_ms?: number;
  fade_out_ms?: number;
  position_anchored_at: number | null;
  position_ms?: number;
  queue?: number[];
  return_to_ambient?: boolean;
};

export type InterruptTemplateCreate = {
  duck_to?: number | null;
  fade_in_ms?: number;
  fade_out_ms?: number;
  name: string;
  playlist?: string | null;
  return_to_ambient?: boolean;
  soundboard_item?: string | null;
};

export type InterruptTemplateUpdate = {
  duck_to?: number | null;
  fade_in_ms?: number | null;
  fade_out_ms?: number | null;
  name?: string | null;
  playlist?: string | null;
  return_to_ambient?: boolean | null;
  soundboard_item?: string | null;
};

export type LibraryContextPassSummary = {
  completed_tracks: number;
  enabled: boolean;
  failed_tracks: number;
  skipped_tracks: number;
  total_tracks: number;
};

export type LibraryContextPasses = {
  audio_context: LibraryContextPassSummary;
  voice_detection: LibraryContextPassSummary;
};

export type LibraryContextStartRequest = {
  force?: boolean;
  scope?: ModelTaggingScope;
};

export type LibraryContextSummary = {
  analyzed_tracks: number;
  analyzer: "local-context/v3";
  failed_tracks: number;
  full_tracks: number;
  last_updated_at: string | null;
  library_tracks: number;
  missing_tracks: number;
  partial_tracks: number;
  passes: LibraryContextPasses;
  stale_tracks: number;
  voice_analyzer: VoiceAnalyzerStatus;
};

export type LibraryConversionRequest = {
  track_ids?: number[] | null;
};

export type LibraryTagPage = {
  items: LibraryTagTrack[];
  limit: number;
  offset: number;
  review_summary: TagReviewSummary;
  total: number;
};

export type LibraryTagTrack = {
  album: string;
  analysis_suggestions: AnalysisTagSuggestionOut[];
  analysis_tags: string[];
  artist: string;
  display_title: string;
  manual_tags: string[];
  model_analysis: {
    context_status: string | null;
    evidence?: string[];
    input_snapshot: Record<string, unknown> | null;
    job_id: string | null;
    status: "current" | "stale" | "missing";
    suggested_tag_count: number | null;
    updated_at_unix_seconds: number | null;
  };
  path: string;
  title: string;
  track_id: number;
};

export type LibraryUploadBody = {
  files: string[];
};

export type LivenessContract = Record<string, string>;

export type LoaderStatus = {
  errors: Record<string, string>;
  last_load_at: number | null;
  loaded_ids: string[];
};

export type LoginRequest = {
  password: string;
  username: string;
};

export type LoopingSfx = {
  id: string;
  interval_s: number;
  item_path: string;
  name: string;
  soundboard_id: string;
  volume?: number;
};

export type ManualTagCatalog = {
  starter_groups: StarterTagGroupOut[];
  tag_usage: ManualTagUsage[];
  used_tags: string[];
};

export type ManualTagPatch = {
  add?: string[];
  remove?: string[];
};

export type ManualTagRenameRequest = {
  source: string;
  target: string;
};

export type ManualTagRenameResult = {
  affected_tracks: number;
  merged: boolean;
  source: string;
  target: string;
};

export type ManualTagUsage = {
  tag: string;
  track_count: number;
};

export type ModeDetail = {
  cues: Record<string, CueSpec>;
  default_crossfade_ms: number;
  default_soundboard: string | null;
  has_theme: boolean;
  id: string;
  integrations: IntegrationsSpec;
  interrupts: InterruptSpec[];
  multiple_outputs: boolean;
  name: string;
  panels: string[];
  playlist_categories: string[];
  presets: Record<string, PresetManifest>;
  soundboards: Record<string, SoundboardManifest>;
};

export type ModeSummary = {
  default_crossfade_ms: number;
  default_soundboard: string | null;
  has_theme: boolean;
  id: string;
  multiple_outputs: boolean;
  name: string;
  panels: string[];
  playlist_categories: string[];
};

export type ModelBatchActionRequest = {
  abandon_uncertain?: boolean;
  cancel?: boolean;
  remote_batch_id?: string | null;
};

export type ModelBatchStatusResponse = {
  id: string;
  input_file_id: string | null;
  remote_batch_id: string | null;
  result?: unknown;
  state: string;
};

export type ModelConformanceOut = {
  contract_version: "assistant-provider-conformance/v3";
  duration_ms: number;
  error_code: string | null;
  finish_reason: string | null;
  input_tokens: number | null;
  output_tokens: number | null;
  passed: boolean;
  provider_model_id: string | null;
  reasoning_tokens: number | null;
  request_settings: ModelRequestSettingsResponse;
  role: ModelRoleOut;
};

export type ModelEqAvailability = {
  available: boolean;
  connection_name: string | null;
  disclosure: ModelEqDisclosure;
  job_kind: string;
  model_id: string | null;
  quality_evaluation_id: "eq-quality-v1";
  reason_code: string | null;
  role_id: "eq_assistant";
};

export type ModelEqDisclosure = {
  may_incur_cost: boolean;
  never_shared: string[];
  shared_with_provider: string[];
  version: "assistant-eq-draft-disclosure/v2";
};

export type ModelEqDraftStartRequest = {
  consent: true;
  disclosure_version: "assistant-eq-draft-disclosure/v2";
  request: EqDraftRequest;
};

export type ModelPlaylistAvailability = {
  available: boolean;
  connection_name: string | null;
  disclosure: ModelPlaylistDisclosure;
  job_kind: string;
  model_id: string | null;
  quality_evaluation_id: "playlist-quality-v1";
  reason_code: string | null;
  role_id: "playlist_planner";
};

export type ModelPlaylistDisclosure = {
  maximum_candidates: number;
  may_incur_cost: boolean;
  never_shared: string[];
  shared_with_provider: string[];
  version: "assistant-playlist-model-disclosure/v4";
};

export type ModelPlaylistSuggestionStartRequest = {
  consent: true;
  disclosure_version: "assistant-playlist-model-disclosure/v4";
  request: PlaylistSuggestionRequest;
};

export type ModelProfileResponse = {
  documented: boolean;
  id: string;
  max_output_tokens: number | null;
  model_ids: string[];
  notice: string;
  reasoning_modes: ThinkingModeWire[];
  revision: string;
  source_url: string;
};

export type ModelQualityEvaluationOut = {
  description: string;
  evaluation_id: string;
  label: string;
  last_evaluated_at: string | null;
  last_job_id: string | null;
  passed_cases: number;
  role_id: string;
  status: "never" | "passed" | "failed" | "stale";
  suite_id: string;
  total_cases: number;
};

export type ModelRequestSettingsResponse = {
  adapter_id: string;
  endpoint_path: string;
  max_output_tokens: number;
  model_id: string;
  model_profile: ModelProfileResponse;
  thinking_mode: ThinkingModeWire;
  timeout_seconds: number;
};

export type ModelRoleDefinitionOut = {
  configuration_available: boolean;
  description: string;
  id: string;
  label: string;
  required_capability_ids: string[];
};

export type ModelRoleOut = {
  configuration_available: boolean;
  conformance_error_code: string | null;
  conformance_status: "never" | "passed" | "failed";
  connection_id: string | null;
  connection_name: string | null;
  description: string;
  effective_enabled: boolean;
  enabled: boolean;
  label: string;
  last_conformance_at: string | null;
  max_output_tokens: number;
  model_id: string;
  required_capability_ids: string[];
  role_id: string;
  thinking_mode: "provider_default" | "enabled" | "disabled" | "low" | "medium" | "high" | "xhigh" | "max";
  timeout_seconds: number;
  updated_at: string | null;
  verification_status: ("never" | "verified" | "failed") | null;
};

export type ModelRoleUpdate = {
  connection_id: string;
  enabled?: boolean;
  max_output_tokens?: number;
  model_id: string;
  thinking_mode?: "provider_default" | "enabled" | "disabled" | "low" | "medium" | "high" | "xhigh" | "max";
  timeout_seconds?: number;
};

export type ModelTagCleanupApplyRequest = {
  catalog_signature: string;
  items: TagCleanupSelectionIn[];
  job_id: string;
  vocabulary_fingerprint: string;
};

export type ModelTagCleanupAvailability = {
  available: boolean;
  catalog_signature: string;
  connection_name: string | null;
  disclosure: ModelTagCleanupDisclosure;
  estimated_provider_requests: number;
  job_kind: string;
  manual_tags: number;
  model_id: string | null;
  quality_evaluation_id: "tag-cleanup-quality-v1";
  reason_code: string | null;
  role_id: "tag_cleanup";
  vocabulary_fingerprint: string;
};

export type ModelTagCleanupDisclosure = {
  maximum_tags: number;
  may_incur_cost: boolean;
  never_shared: string[];
  shared_with_provider: string[];
  version: "assistant-model-tag-cleanup-disclosure/v3";
};

export type ModelTagCleanupStartRequest = {
  consent: true;
  disclosure_version: "assistant-model-tag-cleanup-disclosure/v3";
};

export type ModelTaggingAvailability = {
  available: boolean;
  batch_available: boolean;
  connection_name: string | null;
  current_profiles: number;
  deferred_tracks: number;
  disclosure: ModelTaggingDisclosure;
  estimated_provider_requests: number;
  execution_mode: ModelTaggingExecutionMode;
  job_kind: string;
  library_tracks: number;
  limits: ModelTaggingLimits;
  model_id: string | null;
  pending_batch_id: string | null;
  planned_tracks: number;
  quality_evaluation_id: "music-tagging-quality-v1";
  reason_code: string | null;
  role_id: "music_tagger";
  run_tracks: number;
  scope_tracks: number;
  token_reservation: number;
  tracks_missing_context: number;
  tracks_needing_tags: number;
  tracks_with_full_context: number;
  tracks_with_partial_context: number;
};

export type ModelTaggingDisclosure = {
  allowed_tags: string[];
  invalid_response_retry_limit: number;
  may_incur_cost: boolean;
  never_shared: string[];
  shared_with_provider: string[];
  tracks_per_request: number;
  version: "assistant-model-music-tagging-disclosure/v16";
};

export type ModelTaggingExecutionMode = "standard" | "batch";

export type ModelTaggingLimits = {
  max_requests: number;
  max_token_reservation: number;
  max_tracks: number;
  stop_on_empty_batch: boolean;
};

export type ModelTaggingPlanRequest = {
  context_policy?: "include" | "skip";
  execution_mode?: ModelTaggingExecutionMode;
  force?: boolean;
  limits?: ModelTaggingLimits;
  scope?: ModelTaggingScope;
};

export type ModelTaggingReviewQuery = {
  limit?: number;
  offset?: number;
  review?: "pending" | "accepted" | "rejected";
  scope?: ModelTaggingScope;
};

export type ModelTaggingScope = {
  path?: string;
  recursive?: boolean;
  track_ids?: number[];
  type?: "all" | "folder" | "tracks";
};

export type ModelTaggingStartRequest = {
  consent: true;
  context_policy?: "include" | "skip";
  disclosure_version: "assistant-model-music-tagging-disclosure/v16";
  execution_mode?: ModelTaggingExecutionMode;
  force?: boolean;
  limits?: ModelTaggingLimits;
  scope?: ModelTaggingScope;
};

export type PlayerState = {
  active_mode_id: string | null;
  active_output_device_ids?: string[];
  active_preset_ids?: string[];
  active_soundboard_id: string | null;
  ambient?: AmbientState;
  connected_devices?: DeviceInfo[];
  crossfade_ms?: number;
  crossfade_type?: "linear" | "equal_power" | "cut";
  default_device_volume?: number;
  device_volumes?: Record<string, number>;
  interrupt: InterruptState | null;
  is_playing?: boolean;
  last_position_report: PositionReport | null;
  looping_sfx?: LoopingSfx[];
  multiple_outputs?: boolean;
  position_epoch?: number;
  preset_revision?: number;
  revision?: number;
};

export type PlaylistCandidate = {
  album: string;
  artist: string;
  bpm: number | null;
  confidence: "high" | "medium" | "low";
  default_selected: boolean;
  display_title: string;
  genre: string;
  length_s: number;
  manual_tags: string[];
  match_score: number;
  origin: string;
  path: string;
  planning_energy: number;
  reasons: string[];
  sequence_position: number | null;
  title: string;
  track_id: number;
};

export type PlaylistCreate = {
  category?: string | null;
  mode_id?: string | null;
  name: string;
};

export type PlaylistIntent = {
  brightness: number;
  energy: number;
  matched_moods: string[];
  search_terms: string[];
  tension: number;
};

export type PlaylistMeta = {
  automatic: boolean;
  automatic_refreshed_at: string | null;
  automatic_rule: AutomaticPlaylistRuleV1 | null;
  automatic_rule_error: string | null;
  category: string | null;
  created_at: string;
  id: number;
  mode_id: string | null;
  name: string;
  updated_at: string;
};

export type PlaylistPlan = {
  energy_curve: "steady" | "rising" | "falling" | "arc";
  selected_duration_s: number;
  selected_tracks: number;
};

export type PlaylistSuggestionRequest = {
  candidate_limit?: number;
  energy_curve?: "steady" | "rising" | "falling" | "arc";
  exclude_track_ids?: number[];
  include_unknown_bpm?: boolean;
  max_bpm?: number | null;
  min_bpm?: number | null;
  prompt: string;
  target_minutes?: number;
};

export type PlaylistSuggestionResponse = {
  candidates: PlaylistCandidate[];
  eligible_tracks: number;
  engine: string;
  intent: PlaylistIntent;
  library_tracks: number;
  plan: PlaylistPlan;
};

export type PlaylistUpdate = {
  category?: string | null;
  mode_id?: string | null;
  name?: string | null;
};

export type PositionReport = {
  device_id: string;
  position_ms: number;
  reported_at: number;
};

export type PresetBodyRequest = {
  crossfade_ms?: number | null;
  description?: string | null;
  effects?: EffectSpecIn[];
  name: string;
};

export type PresetManifest = {
  crossfade_ms: number | null;
  description: string | null;
  effects: EffectSpec[];
  id: string;
  name: string;
};

export type ProviderAdapterOut = {
  capability_ids: string[];
  default_model_profile: ModelProfileResponse;
  description: string;
  id: string;
  label: string;
  model_profiles: ModelProfileResponse[];
};

export type ProviderCapabilityOut = {
  description: string;
  id: string;
  label: string;
};

export type ProviderConnectionCreate = {
  adapter_id: string;
  allow_private_network?: boolean;
  api_key: string;
  base_url: string;
  name: string;
};

export type ProviderConnectionOut = {
  adapter_id: string;
  allow_private_network: boolean;
  base_url: string;
  created_at: string;
  credential_saved: boolean;
  id: string;
  key_hint: string | null;
  last_verified_at: string | null;
  name: string;
  updated_at: string;
  verification_error_code: string | null;
  verification_status: "never" | "verified" | "failed";
  verified_capability_ids: string[];
  verified_models: string[];
};

export type ProviderConnectionUpdate = {
  adapter_id?: string | null;
  allow_private_network?: boolean | null;
  api_key?: string | null;
  base_url?: string | null;
  name?: string | null;
};

export type ProviderCredentialStorageReset = {
  current_password: string;
};

export type ProviderCredentialStorageResetOut = {
  deleted_credentials: number;
  master_key_removal_error: string | null;
  master_key_removed: boolean;
  status: ProviderFrameworkStatusOut;
};

export type ProviderFrameworkStatusOut = {
  adapters: ProviderAdapterOut[];
  capabilities: ProviderCapabilityOut[];
  credential_storage_can_initialize: boolean;
  credential_storage_error: string | null;
  credential_storage_host_directory_hint: string | null;
  credential_storage_initialization_error: string | null;
  credential_storage_key_file_path: string | null;
  credential_storage_key_id: string | null;
  credential_storage_ready: boolean;
  credential_storage_source: ("environment" | "file") | null;
  roles: ModelRoleDefinitionOut[];
};

export type ProviderVerificationOut = {
  connection: ProviderConnectionOut;
  error_code: string | null;
  models: string[];
  verified: boolean;
};

export type ReadinessSnapshot = {
  components: Record<string, ComponentStatus>;
  status: ReadinessStatus;
};

export type ReadinessStatus = "starting" | "ready" | "degraded" | "not_ready";

export type ReloadResult = {
  errors: Record<string, string>;
  loaded: string[];
};

export type RescanResult = {
  added: number;
  removed: number;
  unchanged: number;
  updated: number;
};

export type RevertJournalRequest = {
  items: Record<string, unknown>[];
};

export type RevertResult = {
  reverted: number;
  skipped: CleanupSkip[];
};

export type SearchOrder = "asc" | "desc";

export type SearchResponse = {
  limit: number;
  offset: number;
  order: SearchOrder;
  sort: SearchSort;
  total: number;
  tracks: TrackOut[];
};

export type SearchSort = "title" | "artist" | "album" | "album_artist" | "year" | "length_s" | "track_no" | "added_at" | "path";

export type SetActiveModeRequest = {
  mode_id: string | null;
};

export type SfxFileOut = {
  modified_at: string;
  name: string;
  path: string;
  /** True iff some loaded soundboard has an item with this file path. */
  referenced: boolean;
  size_bytes: number;
};

export type SfxFolderOut = {
  file_count: number;
  has_children: boolean;
  name: string;
  path: string;
};

export type SfxFoldersResponse = {
  folders: SfxFolderOut[];
};

export type SfxMoveRequest = {
  /** Destination folder; '' for root. */
  dst_folder: string;
  new_filename?: string | null;
  /** Source path relative to SFX_LIBRARY_DIR. */
  src: string;
};

export type SfxTreeResponse = {
  files: SfxFileOut[];
  path: string;
};

export type SfxUploadBody = {
  files: string[];
};

export type SfxUploadResult = {
  destination: string;
  saved: SfxFileOut[];
  skipped: string[];
};

export type SoundboardCategory = {
  id: string;
  items: SoundboardItem[];
  name: string;
};

export type SoundboardItem = {
  file: string;
  hotkey: string | null;
  icon: string | null;
  name: string;
};

export type SoundboardManifest = {
  categories: SoundboardCategory[];
  id: string;
  name: string | null;
};

export type StarterTagGroupOut = {
  key: string;
  label: string;
  tags: string[];
};

export type TagCleanupApplyRequest = {
  catalog_signature: string;
  items: TagCleanupSelectionIn[];
  vocabulary_fingerprint: string;
};

export type TagCleanupApplyResult = {
  applied: ManualTagRenameResult[];
  catalog_signature: string;
  requested_items: number;
  schema_version: "assistant-tag-cleanup-apply/v1";
};

export type TagCleanupPreviewOut = {
  catalog_signature: string;
  schema_version: "assistant-tag-cleanup-preview/v2";
  suggestions: TagCleanupSuggestionOut[];
  vocabulary_fingerprint: string;
};

export type TagCleanupSelectionIn = {
  source: string;
  target: string;
};

export type TagCleanupSuggestionOut = {
  id: string;
  merged: boolean;
  reason: string;
  reason_code: "vocabulary_alias" | "vocabulary_plural" | "vocabulary_typo";
  source: string;
  source_track_count: number;
  target: string;
  target_track_count: number;
};

export type TagReviewSource = {
  accepted: number;
  analyzer_id: string;
  pending: number;
  rejected: number;
};

export type TagReviewSummary = {
  matching_tracks: number;
  sources: TagReviewSource[];
};

export type TagVocabularyEntry = {
  aliases: string[];
  context_cues: string[];
  description: string;
  id: string;
  name: string;
};

export type TagVocabularyGroup = {
  description: string;
  key: string;
  label: string;
  tags: TagVocabularyEntry[];
};

export type TagVocabularyOut = {
  fingerprint: string;
  groups: TagVocabularyGroup[];
  revision: number;
  schema_version: "assistant-tag-vocabulary/v1";
};

export type TagVocabularyUpdateRequest = {
  expected_revision: number;
  groups: TagVocabularyGroup[];
  schema_version: "assistant-tag-vocabulary/v1";
};

export type ThemeFileResponse = unknown;

export type ThinkingModeWire = "provider_default" | "enabled" | "disabled" | "low" | "medium" | "high" | "xhigh" | "max";

export type TrackAddRequest = {
  position?: number | null;
  track_id: number;
};

export type TrackContextDetail = {
  analyzer_id: "local-context/v3";
  artist: string;
  error: string | null;
  sections: Record<string, unknown>[];
  stages: Record<string, unknown> | null;
  status: "full" | "partial" | "missing" | "stale" | "failed";
  summary: Record<string, unknown> | null;
  technical: Record<string, unknown> | null;
  timeline: Record<string, number>[];
  title: string;
  track_id: number;
  updated_at: string | null;
};

export type TrackInPlaylist = {
  position: number;
  track: {
    album: string;
    artist: string;
    id: number;
    length_s: number;
    path: string;
    title: string;
  } | null;
  track_id: number;
};

export type TrackMetadataUpdate = {
  album?: string | null;
  album_artist?: string | null;
  artist?: string | null;
  bpm?: number | null;
  composer?: string | null;
  disc_no?: number | null;
  display_title?: string | null;
  genre?: string | null;
  origin?: string | null;
  original_release_date?: string | null;
  release_date?: string | null;
  title?: string | null;
  track_no?: number | null;
  year?: number | null;
};

export type TrackMoveRequest = {
  destination: string;
  new_filename?: string | null;
};

export type TrackOut = {
  added_at: string;
  album: string;
  album_artist: string;
  artist: string;
  bpm: number | null;
  composer: string;
  disc_no: number | null;
  display_title: string;
  genre: string;
  id: number;
  length_s: number;
  origin: string;
  original_format: string | null;
  original_release_date: string;
  path: string;
  release_date: string;
  size_bytes: number;
  title: string;
  track_no: number | null;
  year: number | null;
};

export type TreeResponse = {
  path: string;
  tracks: TrackOut[];
};

export type UpdateItemRequest = {
  file?: string | null;
  hotkey?: string | null;
  icon?: string | null;
  name?: string | null;
};

/** Mode settings edited in place; omitted fields keep their current value. */
export type UpdateModeRequest = {
  multiple_outputs?: boolean | null;
  name?: string | null;
};

export type UploadCheckItem = {
  dest: string;
  name: string;
};

export type UploadCheckRequest = {
  items: UploadCheckItem[];
};

export type UploadCheckResponse = {
  collisions: UploadCheckItem[];
};

export type UploadResult = {
  conversion_job_id: string | null;
  destination: string;
  saved: TrackOut[];
  skipped: string[];
};

export type UserInfoResponse = {
  id: number;
  username: string;
};

export type ValidationError = {
  ctx?: Record<string, unknown>;
  input?: unknown;
  loc: (string | number)[];
  msg: string;
  type: string;
};

export type VerifyRequest = {
  names: string[];
};

export type VerifyResult = {
  failed: string[];
  verified: number;
};

export type VoiceAnalyzerStatus = {
  analyzer_id: "essentia-musicnn-voice/v2";
  model_filename: string;
  model_sha256: string;
  reason: ("model_missing" | "model_unreadable" | "unsupported_model" | "runtime_missing") | null;
  status: "not_configured" | "ready" | "unavailable";
};

export type app__api__playlists__TrackMoveRequest = {
  to_position: number;
};

export type app__api__sfx__FolderCreateRequest = {
  /** Folder path relative to SFX_LIBRARY_DIR. */
  path: string;
};

export type app__api__sfx__FolderDeleteResult = {
  deleted: boolean;
};

export type app__api__sfx__FolderRenameRequest = {
  dst: string;
  src: string;
};
