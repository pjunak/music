// WebSocket protocol bindings are generated from the authoritative Rust DTOs.
// Runtime validation remains in wsValidate.ts because TypeScript types vanish
// at the browser boundary.
export type {
  AmbientState,
  CrossfadeType,
  DeviceInfo,
  InterruptState,
  LoopMode,
  LoopingSfx,
  PlayerState,
  PositionReport,
  ShuffleMode,
  WsAction,
  WsMessage,
} from "@/generated/protocol";

/** A remembered device from the operator's persistent registry
 *  (`GET /api/devices`). */
export interface KnownDevice {
  client_id: string;
  name: string;
  is_output: boolean;
  connected: boolean;
  added_at: string | null;
}

// Library track shape returned by /api/library/*.
export interface Track {
  id: number;
  path: string;
  title: string;
  artist: string;
  album_artist: string;
  album: string;
  track_no: number | null;
  disc_no: number | null;
  year: number | null;
  genre: string;
  length_s: number;
  bpm: number | null;
  size_bytes: number;
  added_at: string;
  // User-entered, DB-only labels — see backend/app/models/track.py.
  display_title: string;
  origin: string;
}

// Compact track summary returned in playlist listings.
export interface TrackSummary {
  id: number;
  path: string;
  title: string;
  artist: string;
  album: string;
  length_s: number;
}

// EQ preset shape — per-mode effect rack. Effects are loosely-typed (each
// effect carries a `type` plus type-specific params validated server-side).
export interface PresetEffect {
  type: string;
  [key: string]: unknown;
}

export interface PresetManifest {
  id: string;
  name: string;
  description?: string;
  effects: PresetEffect[];
  crossfade_ms?: number | null;
}

// Mode summary shape returned by /api/modes.
export interface ModeSummary {
  id: string;
  name: string;
  panels: string[];
  playlist_categories: string[];
  has_theme: boolean;
  default_crossfade_ms: number;
  default_soundboard: string | null;
}

// Detail shape returned by /api/modes/{id}.
export interface SoundboardItem {
  file: string;
  name: string;
  icon?: string | null;
  hotkey?: string | null;
}

export interface SoundboardCategory {
  id: string;
  name: string;
  items: SoundboardItem[];
}

export interface SoundboardManifest {
  id: string;
  name?: string | null;
  categories: SoundboardCategory[];
}

export interface InterruptSpec {
  name: string;
  playlist?: string | null;
  soundboard_item?: string | null;
  fade_in_ms?: number;
  fade_out_ms?: number;
  return_to_ambient?: boolean;
  /** Ambient duck level during the interrupt (0..1). Null = pause. */
  duck_to?: number | null;
}

export interface CueSfx {
  soundboard: string;
  item: string;
  volume?: number;
}

export interface CueLoop {
  soundboard: string;
  item: string;
  interval_s: number;
  volume?: number;
}

/** A saved one-click setup: apply a preset, start a playlist (from a song +
 *  timestamp), fire one-shot SFX, start loops. Mode-scoped. */
export interface Cue {
  id: string;
  name: string;
  description?: string | null;
  preset?: string | null;
  playlist?: string | null;
  start_index?: number;
  start_ms?: number;
  sfx?: CueSfx[];
  loops?: CueLoop[];
}

export interface ModeDetail extends ModeSummary {
  interrupts: InterruptSpec[];
  integrations: { lights?: unknown };
  soundboards: Record<string, SoundboardManifest>;
  cues: Record<string, Cue>;
}

// Playlist meta shape returned by /api/playlists.
export interface AutomaticPlaylistRule {
  schema: "automatic-playlist/v1";
  include_tags: string[];
  match: "any" | "all";
  exclude_tags: string[];
  tag_sources: "manual" | "manual_and_local";
  min_bpm: number | null;
  max_bpm: number | null;
  include_unknown_bpm: boolean;
  maximum_tracks: number;
  order_by: "title" | "newest" | "bpm_ascending" | "bpm_descending";
}

export interface PlaylistMeta {
  id: number;
  name: string;
  mode_id: string | null;
  category: string | null;
  automatic: boolean;
  automatic_rule: AutomaticPlaylistRule | null;
  automatic_rule_error: string | null;
  automatic_refreshed_at: string | null;
  created_at: string;
  updated_at: string;
}

// Track-in-playlist shape returned by /api/playlists/{id}/tracks.
export interface TrackInPlaylist {
  position: number;
  track_id: number;
  track: TrackSummary | null;
}

export interface AutomaticPlaylistTrack extends TrackSummary {
  bpm: number | null;
}

export interface AutomaticPlaylistPreview {
  schema_version: "automatic-playlist-preview/v1";
  source_signature: string;
  library_tracks: number;
  matched_tracks: number;
  tracks: AutomaticPlaylistTrack[];
}

export interface AutomaticPlaylistApplyResult {
  schema_version: "automatic-playlist-apply/v1";
  playlist: PlaylistMeta;
  materialized_tracks: number;
}

export interface FolderEntry {
  name: string;
  path: string;
  track_count: number;
  /** True iff this folder has at least one subfolder. Used by the tree UI
   *  to hide the expand toggle on leaf folders. */
  has_children: boolean;
}

// Tracks immediately in one folder, from /api/library/tree. The folder
// hierarchy itself comes from /api/library/folders (FoldersResponse).
export interface TreeResponse {
  path: string;
  tracks: Track[];
}

// Whole-hierarchy listing from /api/library/folders — every folder at any
// depth in one response, so the tree UI can filter/reveal client-side.
export interface FoldersResponse {
  folders: FolderEntry[];
}
