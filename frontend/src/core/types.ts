// Protocol types mirroring backend/app/sync/protocol.py.
// Source of truth for the contract: backend/app/sync/protocol.py.

export type LoopMode = "off" | "queue" | "track";

export interface AmbientState {
  current_beets_id: number | null;
  queue: number[];
  history: number[];
  position_ms: number;
  loop: LoopMode;
}

export interface InterruptState {
  current_beets_id: number;
  queue: number[];
  position_ms: number;
  return_to_ambient: boolean;
  fade_in_ms: number;
  fade_out_ms: number;
}

export interface PositionReport {
  device_id: string;
  position_ms: number;
  reported_at: number;
}

export interface DeviceInfo {
  device_id: string;
  name: string;
  capabilities: string[];
}

export interface PlayerState {
  revision: number;
  is_playing: boolean;
  volume: number;
  active_mode_id: string | null;
  active_output_device_ids: string[];
  active_soundboard_id: string | null;
  active_preset_ids: string[];
  active_scene_id: string | null;
  crossfade_ms: number;
  crossfade_type: string;
  ambient: AmbientState;
  interrupt: InterruptState | null;
  last_position_report: PositionReport | null;
  connected_devices: DeviceInfo[];
}

// Library track shape returned by /api/library/*.
export interface Track {
  beets_id: number;
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
  path: string;
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

// Playlist meta shape returned by /api/playlists.
export interface PlaylistMeta {
  id: number;
  name: string;
  mode_id: string | null;
  category: string | null;
  source: "manual" | "smart";
  rules_json: Record<string, unknown> | null;
  created_at: string;
  updated_at: string;
}

// Track-in-playlist shape returned by /api/playlists/{id}/tracks.
export interface TrackInPlaylist {
  position: number;
  beets_id: number;
  display_name: string | null;
  track: Track | null;
}

// --- WebSocket actions (client → server) ---------------------------------

export type WsAction =
  | { type: "register"; name: string; capabilities: string[] }
  | { type: "set_volume"; volume: number }
  | { type: "pause" }
  | { type: "resume" }
  | { type: "set_active_mode"; mode_id: string | null }
  | { type: "set_active_outputs"; device_ids: string[] }
  | { type: "position_report"; position_ms: number }
  | { type: "ambient_play_track"; beets_id: number }
  | { type: "ambient_set_queue"; beets_ids: number[] }
  | { type: "ambient_enqueue"; beets_id: number; position?: number }
  | { type: "ambient_clear_queue" }
  | { type: "ambient_skip_next" }
  | { type: "ambient_skip_prev" }
  | { type: "ambient_seek"; position_ms: number }
  | { type: "ambient_set_loop"; loop: LoopMode }
  | { type: "ambient_stop" }
  | { type: "ambient_play_playlist"; playlist_id: number; start_index?: number }
  | { type: "set_active_soundboard"; soundboard_id: string | null }
  | { type: "set_active_presets"; preset_ids: string[] }
  | { type: "set_crossfade"; crossfade_ms: number; crossfade_type?: string }
  | {
      type: "fire_interrupt_track";
      beets_id: number;
      return_to_ambient?: boolean;
      fade_in_ms?: number;
      fade_out_ms?: number;
    }
  | {
      type: "fire_interrupt_playlist";
      playlist_id: number;
      return_to_ambient?: boolean;
      fade_in_ms?: number;
      fade_out_ms?: number;
    }
  | { type: "interrupt_skip_next" }
  | { type: "interrupt_seek"; position_ms: number }
  | { type: "cancel_interrupt" }
  | { type: "fire_sfx"; soundboard_id: string; item_path: string; volume?: number }
  | { type: "activate_scene"; scene_id: string }
  | { type: "deactivate_scene" };

// --- WebSocket events (server → client) ----------------------------------

export type WsMessage =
  | { type: "state_snapshot"; your_device_id: string; state: PlayerState }
  | { type: "state_changed"; state: PlayerState }
  | { type: "sfx_fired"; soundboard_id: string; item_path: string; volume: number }
  | {
      type: "scene_activated";
      scene_id: string;
      mode_id: string;
      scene: Record<string, unknown>;
    }
  | { type: "scene_deactivated"; scene_id: string; mode_id: string | null }
  | { type: "error"; detail: string };
