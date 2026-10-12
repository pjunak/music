import type { Cue, InterruptSpec, ModeDetail, ModeSummary, PresetEffect, PresetManifest, SoundboardManifest } from "@/core/types";
import type * as Http from "@/generated/http";

import { api } from "./http";

export type { PresetManifest, PresetEffect } from "@/core/types";

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

export const presetsApi = {
  // Presets are per-mode now — list the given mode's EQ presets.
  list: (modeId: string) =>
    api.get<PresetManifest[]>(
      `/api/modes/${encodeURIComponent(modeId)}/presets`,
    ),
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
