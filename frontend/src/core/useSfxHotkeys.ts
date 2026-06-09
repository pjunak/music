import { useEffect, useState } from "react";

import { modesApi } from "@/core/api";
import { isInteractiveTarget } from "@/core/isInteractiveTarget";
import { usePlayerStore } from "@/core/playerStore";
import type { ModeDetail } from "@/core/types";
import { useUiStore } from "@/core/uiStore";
import { wsClient } from "@/core/ws";

/** When the user has an active mode + soundboard, bind any keyboard
 *  hotkeys declared on soundboard items so the DM can trigger SFX from
 *  anywhere in the app. Skips when the user is typing. */
export function useSfxHotkeys(): void {
  const activeModeId = usePlayerStore((s) => s.state?.active_mode_id ?? null);
  const activeSoundboardId = usePlayerStore(
    (s) => s.state?.active_soundboard_id ?? null,
  );

  const [mode, setMode] = useState<ModeDetail | null>(null);

  useEffect(() => {
    if (activeModeId === null) {
      setMode(null);
      return;
    }
    let cancelled = false;
    void modesApi
      .get(activeModeId)
      .then((m) => {
        if (!cancelled) setMode(m);
      })
      .catch(() => {
        if (!cancelled) setMode(null);
      });
    return () => {
      cancelled = true;
    };
  }, [activeModeId]);

  useEffect(() => {
    if (mode === null || activeSoundboardId === null) return;
    const soundboard = mode.soundboards[activeSoundboardId];
    if (!soundboard) return;

    // Build map: lowercased single-char hotkey → item.file
    const bindings = new Map<string, string>();
    for (const cat of soundboard.categories) {
      for (const item of cat.items) {
        if (typeof item.hotkey === "string" && item.hotkey.length === 1) {
          bindings.set(item.hotkey.toLowerCase(), item.file);
        }
      }
    }
    if (bindings.size === 0) return;

    function onKey(e: KeyboardEvent) {
      if (e.metaKey || e.ctrlKey || e.altKey) return;
      if (isInteractiveTarget(e.target)) return;
      const key = e.key.toLowerCase();
      const itemPath = bindings.get(key);
      if (itemPath === undefined) return;
      e.preventDefault();
      wsClient.send({
        type: "fire_sfx",
        soundboard_id: activeSoundboardId as string,
        item_path: itemPath,
        volume: useUiStore.getState().sfxVolume,
      });
    }

    window.addEventListener("keydown", onKey);
    return () => window.removeEventListener("keydown", onKey);
  }, [mode, activeSoundboardId]);
}
