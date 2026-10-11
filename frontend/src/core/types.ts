import type * as Http from "@/generated/http";

// WebSocket and HTTP types are generated from the authoritative Rust DTOs; the
// names below are the frontend's aliases for them. WebSocket frames are also
// validated at runtime in wsValidate.ts because types vanish at that boundary.
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
export type KnownDevice = Http.DeviceOut;

// Library track shape returned by /api/library/*.
export type Track = Http.TrackOut;

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
export type PresetEffect = Http.EffectSpec;

export type PresetManifest = Http.PresetManifest;

// Mode summary shape returned by /api/modes.
export type ModeSummary = Http.ModeSummary;

// Detail shape returned by /api/modes/{id}.
export type SoundboardItem = Http.SoundboardItem;

export type SoundboardCategory = Http.SoundboardCategory;

export type SoundboardManifest = Http.SoundboardManifest;

export type InterruptSpec = Http.InterruptSpec;

export type CueSfx = Http.CueSfx;

export type CueLoop = Http.CueLoop;
/** Editable cue entries; the server defaults a missing volume to 1. */
export type CueSfxInput = Http.CueSfxIn;
export type CueLoopInput = Http.CueLoopIn;
export type CueBody = Http.CueBody;

/** A saved one-click setup: apply a preset, start a playlist (from a song +
 *  timestamp), fire one-shot SFX, start loops. Mode-scoped. */
export type Cue = Http.CueSpec;

export interface ModeDetail extends ModeSummary {
  interrupts: InterruptSpec[];
  integrations: { lights?: unknown };
  soundboards: Record<string, SoundboardManifest>;
  cues: Record<string, Cue>;
}

// Playlist meta shape returned by /api/playlists.
export type AutomaticPlaylistRule = Http.AutomaticPlaylistRuleV1;

export type PlaylistMeta = Http.PlaylistMeta;

// Track-in-playlist shape returned by /api/playlists/{id}/tracks.
export type TrackInPlaylist = Http.TrackInPlaylist;

export interface AutomaticPlaylistTrack extends TrackSummary {
  bpm: number | null;
}

export type AutomaticPlaylistPreview = Http.AutomaticPlaylistPreview;

export type AutomaticPlaylistApplyResult = Http.AutomaticPlaylistApplyResult;

export type FolderEntry = Http.FolderOut;

// Tracks immediately in one folder, from /api/library/tree. The folder
// hierarchy itself comes from /api/library/folders (FoldersResponse).
export type TreeResponse = Http.TreeResponse;

// Whole-hierarchy listing from /api/library/folders — every folder at any
// depth in one response, so the tree UI can filter/reveal client-side.
export type FoldersResponse = Http.FoldersResponse;
