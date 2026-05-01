import { create } from "zustand";
import { persist } from "zustand/middleware";

export type TabKey = "player" | "library" | "controls" | "settings";

export type Capability = "controls" | "audio_output";

interface UiStore {
  /** Hide the cover art on the Player tab. The DM may want a black screen
   *  on the room display while running a session. Persisted to localStorage. */
  hidePlayerArt: boolean;
  setHidePlayerArt: (v: boolean) => void;

  /** Display name registered with the server for this browser tab.
   *  `null` = use the auto-detected default (Phone / Browser). */
  deviceName: string | null;
  setDeviceName: (name: string | null) => void;

  /** Capabilities this tab announces. Default: both. Toggling changes the
   *  registered capabilities on the next register call. */
  capabilities: Capability[];
  setCapabilities: (caps: Capability[]) => void;

  /** Local-only override that forces the playback engine to treat this
   *  device as an active output regardless of `active_output_device_ids`.
   *  Lets guests (who can't mutate server state) still hear audio on this
   *  tab — used by the Player tab's "Play on this device" toggle. */
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
      capabilities: ["controls", "audio_output"],
      setCapabilities: (caps) => set({ capabilities: caps }),
      forceLocalPlayback: false,
      setForceLocalPlayback: (v) => set({ forceLocalPlayback: v }),
      sfxVolume: 0.8,
      setSfxVolume: (v) => set({ sfxVolume: Math.max(0, Math.min(1, v)) }),
    }),
    { name: "music-ui" },
  ),
);
