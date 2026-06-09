import { create } from "zustand";
import { persist } from "zustand/middleware";

/** Default SFX volume (0–1) used both for the store initializer and as the
 *  fire-time fallback when no per-device override has been set. */
export const DEFAULT_SFX_VOLUME = 0.8;

/** Stable per-install identity. Generated once and persisted, so the server
 *  can recognise this browser across refreshes/restarts and the operator's
 *  audio-output designation sticks to it. Falls back to a non-crypto id in
 *  non-secure contexts (e.g. an old TV reached over plain http on a LAN),
 *  where `crypto.randomUUID` is unavailable. */
function generateClientId(): string {
  try {
    if (typeof crypto !== "undefined" && typeof crypto.randomUUID === "function") {
      return crypto.randomUUID();
    }
  } catch {
    /* fall through to the non-crypto path */
  }
  return `cid-${Math.random().toString(36).slice(2)}${Date.now().toString(36)}`;
}

interface UiStore {
  /** Hide the cover art on the Player tab. The DM may want a black screen
   *  on the room display while running a session. Persisted to localStorage. */
  hidePlayerArt: boolean;
  setHidePlayerArt: (v: boolean) => void;

  /** Display name registered with the server for this browser tab.
   *  `null` = use the auto-detected default (Phone / Browser). */
  deviceName: string | null;
  setDeviceName: (name: string | null) => void;

  /** Stable identity sent in every `register`. Persisted; never changes for
   *  this browser once generated. */
  clientId: string;

  /** Local-only override that forces the playback engine to treat this device
   *  as an active output regardless of `active_output_device_ids`. Lets a
   *  guest (who can't mutate server state) hear audio on this tab. Deliberately
   *  NOT persisted — output is fully manual, so a refresh never auto-resumes
   *  local playback. */
  forceLocalPlayback: boolean;
  setForceLocalPlayback: (v: boolean) => void;

  /** SFX volume (0–1) for the live Soundboard panel — preserved across
   *  tab switches so the operator doesn't reset to the default every
   *  time they leave Controls. Per-device, not synced server-side. */
  sfxVolume: number;
  setSfxVolume: (v: number) => void;
}

export function defaultDeviceName(): string {
  if (typeof navigator === "undefined") return "Browser";
  const ua = navigator.userAgent;

  let platform: string;
  if (/iPhone/i.test(ua)) platform = "iPhone";
  else if (/iPad/i.test(ua)) platform = "iPad";
  else if (/Android/i.test(ua))
    platform = /Mobile/i.test(ua) ? "Android phone" : "Android tablet";
  else if (/Windows/i.test(ua)) platform = "Windows PC";
  else if (/Macintosh|Mac OS X/i.test(ua)) platform = "Mac";
  else if (/Linux|X11/i.test(ua)) platform = "Linux";
  else platform = "Browser";

  // Order matters: Edge/Opera both contain "Chrome" in their UA, so they
  // have to be checked first. Safari's UA contains the bare word but never
  // "Chrome" or "Chromium".
  let browser: string | null = null;
  if (/Edg\//i.test(ua)) browser = "Edge";
  else if (/OPR\/|Opera\//i.test(ua)) browser = "Opera";
  else if (/Firefox\/|FxiOS\//i.test(ua)) browser = "Firefox";
  else if (/Chrome\//i.test(ua) && !/Chromium/i.test(ua)) browser = "Chrome";
  else if (/Safari\//i.test(ua)) browser = "Safari";

  return browser !== null ? `${platform} · ${browser}` : platform;
}

export const useUiStore = create<UiStore>()(
  persist(
    (set) => ({
      hidePlayerArt: false,
      setHidePlayerArt: (v) => set({ hidePlayerArt: v }),
      deviceName: null,
      setDeviceName: (name) => set({ deviceName: name }),
      clientId: generateClientId(),
      forceLocalPlayback: false,
      setForceLocalPlayback: (v) => set({ forceLocalPlayback: v }),
      sfxVolume: DEFAULT_SFX_VOLUME,
      setSfxVolume: (v) => set({ sfxVolume: Math.max(0, Math.min(1, v)) }),
    }),
    {
      name: "music-ui",
      version: 1,
      // Only persist durable prefs. `forceLocalPlayback` is intentionally
      // omitted so it's session-only (no auto-resume on refresh).
      partialize: (s) => ({
        hidePlayerArt: s.hidePlayerArt,
        deviceName: s.deviceName,
        clientId: s.clientId,
        sfxVolume: s.sfxVolume,
      }),
      migrate: (persisted) => {
        // v0 → v1: drop the old self-asserted `capabilities` (output is now a
        // server-side designation) and any persisted `forceLocalPlayback`
        // (now session-only). Anything left merges over the defaults, so a
        // missing clientId is backfilled by the initializer.
        const p = { ...(persisted as Record<string, unknown> | null) };
        delete p.capabilities;
        delete p.forceLocalPlayback;
        return p as unknown as UiStore;
      },
    },
  ),
);
