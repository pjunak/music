import { useEffect } from "react";
import { useNavigate } from "react-router-dom";

import { useAuthStore } from "@/core/auth";
import { isInteractiveTarget } from "@/core/isInteractiveTarget";
import { usePlayerStore } from "@/core/playerStore";
import { useUiTransient } from "@/core/uiTransient";
import { wsClient } from "@/core/ws";

/** Global shortcuts wired once per session at the AppShell.
 *
 *  Conventions:
 *  - Space: toggle play / pause
 *  - ← / → : prev / next track
 *  - L : cycle ambient loop mode
 *  - / : focus the library search box
 *  - ? (Shift+/) : open the keyboard-shortcut sheet
 *  - 1–4 : switch top-level tabs (Console, Library, Authoring, Settings).
 *          The TV route at `/` isn't reachable by number — it's the guest
 *          landing, not part of the authed tab strip. Sub-tabs (Authoring
 *          Playlists/Soundboards/Interrupts/EQ Presets/Cues; Library has none)
 *          are click-only — adding numbers for them would crowd the shortcut
 *          space and the sub-strip is right there.
 *  - Esc : already handled per-modal
 *
 *  All shortcuts no-op when the user is typing (input, textarea, etc.) so
 *  Space in the search box still types a space.
 */
export function useKeyboardShortcuts(): void {
  const navigate = useNavigate();

  useEffect(() => {
    function onKey(e: KeyboardEvent) {
      if (isInteractiveTarget(e.target)) return;
      if (e.metaKey || e.ctrlKey || e.altKey) return;

      const state = usePlayerStore.getState().state;
      // Guests can't mutate playback state — the server rejects these
      // actions and the rejection surfaces as an error toast. No-op instead.
      const isAuthed =
        useAuthStore.getState().status === "authenticated";

      switch (e.key) {
        case " ": {
          if (!isAuthed) return;
          if (state === null) return;
          e.preventDefault();
          wsClient.send({ type: state.is_playing ? "pause" : "resume" });
          return;
        }
        case "ArrowRight":
          if (!isAuthed) return;
          e.preventDefault();
          wsClient.send({ type: "ambient_skip_next" });
          return;
        case "ArrowLeft":
          if (!isAuthed) return;
          e.preventDefault();
          wsClient.send({ type: "ambient_skip_prev" });
          return;
        case "l":
        case "L": {
          if (!isAuthed) return;
          if (state === null) return;
          // off → single (track) → whole queue, matching the footer's
          // repeat-cycle button order.
          const order = ["off", "track", "queue"] as const;
          const idx = order.indexOf(state.ambient.loop);
          const next = order[(idx + 1) % order.length];
          wsClient.send({ type: "ambient_set_loop", loop: next });
          return;
        }
        case "/":
          e.preventDefault();
          {
            // Selector targets the library toolbar's always-visible search
            // input. We navigate first (in case the user is on a different
            // tab) and focus on the next tick, after the route render.
            navigate("/library");
            window.setTimeout(() => {
              const el = document.querySelector<HTMLInputElement>(
                ".library-toolbar-search input[type=search]",
              );
              el?.focus();
              el?.select();
            }, 0);
          }
          return;
        case "?":
          // Shift+/ on US layouts. Other layouts will route through
          // `e.key === "?"` too since the key value carries the produced
          // character, not the physical position.
          e.preventDefault();
          useUiTransient.getState().setShortcutSheetOpen(true);
          return;
        case "1":
          e.preventDefault();
          navigate("/console");
          return;
        case "2":
          e.preventDefault();
          navigate("/library");
          return;
        case "3":
          e.preventDefault();
          navigate("/authoring");
          return;
        case "4":
          e.preventDefault();
          navigate("/settings");
          return;
      }
    }

    window.addEventListener("keydown", onKey);
    return () => window.removeEventListener("keydown", onKey);
  }, [navigate]);
}
