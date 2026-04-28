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
}

export function defaultDeviceName(): string {
  if (typeof navigator === "undefined") return "Browser";
  if (/Mobile|Android|iPhone/i.test(navigator.userAgent)) return "Phone";
  return "Browser";
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
    }),
    { name: "music-ui" },
  ),
);
