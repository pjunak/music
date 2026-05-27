import { useEffect } from "react";
import { useNavigate } from "react-router-dom";

import { usePlayerStore } from "@/core/playerStore";
import { useUiTransient } from "@/core/uiTransient";
import { wsClient } from "@/core/ws";

/** Returns true if the event originated in a place where shortcuts should
 *  not preempt typing OR steal arrow-key seeks from a focused custom
 *  control. Inputs, textareas, contenteditable, native form controls, and
 *  ARIA sliders (the seek bar is a tabindex'd <div role="slider"> with its
 *  own onKeyDown handler — we don't want the global ←/→ to also fire
 *  prev/next while the operator is scrubbing). */
function isInteractiveTarget(target: EventTarget | null): boolean {
  if (!(target instanceof HTMLElement)) return false;
  const tag = target.tagName;
  if (tag === "INPUT" || tag === "TEXTAREA" || tag === "SELECT") return true;
  if (target.isContentEditable) return true;
  if (target.getAttribute("role") === "slider") return true;
  return false;
}

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
 *          landing, not part of the authed tab strip. Sub-tabs (Library
 *          Files/Tags, Authoring Playlists/Soundboards/Modes/Presets) are
 *          click-only — adding numbers for them would crowd the shortcut
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

      switch (e.key) {
        case " ": {
          if (state === null) return;
          e.preventDefault();
          wsClient.send({ type: state.is_playing ? "pause" : "resume" });
          return;
        }
        case "ArrowRight":
          e.preventDefault();
          wsClient.send({ type: "ambient_skip_next" });
          return;
        case "ArrowLeft":
          e.preventDefault();
          wsClient.send({ type: "ambient_skip_prev" });
          return;
        case "l":
        case "L": {
          if (state === null) return;
          const order = ["off", "queue", "track"] as const;
          const idx = order.indexOf(state.ambient.loop);
          const next = order[(idx + 1) % order.length];
          wsClient.send({ type: "ambient_set_loop", loop: next });
          return;
        }
        case "/":
          e.preventDefault();
          {
            const el = document.querySelector<HTMLInputElement>(
              ".library-search input[type=search]",
            );
            if (el) {
              navigate("/library");
              window.setTimeout(() => el.focus(), 0);
            }
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
