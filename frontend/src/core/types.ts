// Protocol types mirroring backend/app/sync/protocol.py.
// Source of truth for the contract: backend/app/sync/protocol.py.

// "follow" continues into the rest of the library once the queue ends — the
// "Continue / Autoplay (∞)" mode. Mutually exclusive with the repeat modes.
export type LoopMode = "off" | "follow" | "queue" | "track";
export type ShuffleMode = "off" | "random" | "weighted";

export interface AmbientState {
  current_track_id: number | null;
  queue: number[];
  history: number[];
  position_ms: number;
  loop: LoopMode;
  shuffle: ShuffleMode;
  /** The playlist the lane was started from (for the Console's "now driving"
   *  highlight), or null once the queue diverges. */
  source_playlist_id: number | null;
}

export interface InterruptState {
  current_track_id: number;
  queue: number[];
  position_ms: number;
  return_to_ambient: boolean;
  fade_in_ms: number;
  fade_out_ms: number;
  /** When non-null (0..1), ambient music keeps playing at this volume
   *  multiplier during the interrupt instead of pausing. Drives the
   *  cinematic "duck under voiceover" effect. */
  duck_to: number | null;
}

export interface PositionReport {
  device_id: string;
  position_ms: number;
  reported_at: number;
}

/** A repeating SFX driven by a server-side interval timer. Shown in the
 *  Console LOOPS panel; `id` is the stop handle. */
export interface LoopingSfx {
  id: string;
  name: string;
  soundboard_id: string;
  item_path: string;
  interval_s: number;
  volume: number;
}

export interface DeviceInfo {
  /** Same value as `client_id` (the stable identity) — kept so existing
   *  code that keys on `device_id` keeps working. */
  device_id: string;
  client_id: string;
  name: string;
  /** Whether this device is a designated audio output (persistent registry). */
  is_output: boolean;
}

/** A remembered device from the operator's persistent registry
 *  (`GET /api/devices`). */
export interface KnownDevice {
  client_id: string;
  name: string;
  is_output: boolean;
  connected: boolean;
  added_at: string | null;
}

export interface PlayerState {
  revision: number;
  is_playing: boolean;
  volume: number;
  active_mode_id: string | null;
  active_output_device_ids: string[];
  /** Per-device volume trim (client_id → 0..1), applied on top of master
   *  `volume` on that device only. Absent entry = 1.0 (no trim). */
  device_volumes: Record<string, number>;
  active_soundboard_id: string | null;
  active_preset_ids: string[];
  crossfade_ms: number;
  crossfade_type: string;
  ambient: AmbientState;
  interrupt: InterruptState | null;
  /** Repeating SFX currently running (server-timer driven). */
  looping_sfx: LoopingSfx[];
  last_position_report: PositionReport | null;
  connected_devices: DeviceInfo[];
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
  volume?: number | null;
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
export interface PlaylistMeta {
  id: number;
  name: string;
  mode_id: string | null;
  category: string | null;
  created_at: string;
  updated_at: string;
}

// Track-in-playlist shape returned by /api/playlists/{id}/tracks.
export interface TrackInPlaylist {
  position: number;
  track_id: number;
  track: TrackSummary | null;
}

// Library tree shape returned by /api/library/tree.
export interface FolderEntry {
  name: string;
  path: string;
  track_count: number;
  /** True iff this folder has at least one subfolder. Used by the tree UI
   *  to hide the expand toggle on leaf folders. */
  has_children: boolean;
}

export interface TreeResponse {
  path: string;
  folders: FolderEntry[];
  tracks: Track[];
}

// Whole-hierarchy listing from /api/library/folders — every folder at any
// depth in one response, so the tree UI can filter/reveal client-side.
export interface FoldersResponse {
  folders: FolderEntry[];
}

// --- WebSocket actions (client → server) ---------------------------------

export type WsAction =
  | { type: "register"; name: string; client_id: string }
  | { type: "set_volume"; volume: number }
  | { type: "pause" }
  | { type: "resume" }
  | { type: "set_active_mode"; mode_id: string | null }
  | { type: "set_active_outputs"; device_ids: string[] }
  | { type: "set_device_volume"; device_id: string; volume: number }
  | { type: "position_report"; position_ms: number }
  | { type: "ambient_play_track"; track_id: number }
  | { type: "ambient_set_queue"; track_ids: number[] }
  | { type: "ambient_enqueue"; track_id: number; position?: number }
  | { type: "ambient_clear_queue" }
  | { type: "ambient_skip_next" }
  | { type: "ambient_skip_prev" }
  | { type: "ambient_seek"; position_ms: number }
  | { type: "ambient_set_loop"; loop: LoopMode }
  | { type: "ambient_set_shuffle"; shuffle: ShuffleMode }
  | { type: "ambient_stop" }
  | { type: "ambient_play_playlist"; playlist_id: number; start_index?: number }
  | { type: "ambient_play_folder"; path: string; start_index?: number }
  | { type: "set_active_soundboard"; soundboard_id: string | null }
  | { type: "set_active_presets"; preset_ids: string[] }
  | { type: "set_crossfade"; crossfade_ms: number; crossfade_type?: string }
  | {
      type: "fire_interrupt_track";
      track_id: number;
      return_to_ambient?: boolean;
      fade_in_ms?: number;
      fade_out_ms?: number;
      duck_to?: number | null;
    }
  | {
      type: "fire_interrupt_playlist";
      playlist_id: number;
      return_to_ambient?: boolean;
      fade_in_ms?: number;
      fade_out_ms?: number;
      duck_to?: number | null;
    }
  | { type: "interrupt_skip_next" }
  | { type: "interrupt_seek"; position_ms: number }
  | { type: "cancel_interrupt" }
  | { type: "fire_sfx"; soundboard_id: string; item_path: string; volume?: number }
  | {
      type: "start_loop";
      id: string;
      name: string;
      soundboard_id: string;
      item_path: string;
      interval_s: number;
      volume?: number;
    }
  | { type: "stop_loop"; id: string }
  | { type: "fire_cue"; cue_id: string };

// --- WebSocket events (server → client) ----------------------------------

export type WsMessage =
  | { type: "state_snapshot"; your_device_id: string; state: PlayerState }
  | { type: "state_changed"; state: PlayerState }
  | { type: "sfx_fired"; soundboard_id: string; item_path: string; volume: number }
  | { type: "error"; detail: string };
