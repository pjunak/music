import { useEffect } from "react";
import { useNavigate } from "react-router-dom";

import { usePlayerStore } from "@/core/playerStore";
import { wsClient } from "@/core/ws";

/** Returns true if the event originated in a place where shortcuts should
 *  not preempt typing. Inputs, textareas, contenteditable, and forms etc. */
function isTypingTarget(target: EventTarget | null): boolean {
  if (!(target instanceof HTMLElement)) return false;
  const tag = target.tagName;
  if (tag === "INPUT" || tag === "TEXTAREA" || tag === "SELECT") return true;
  if (target.isContentEditable) return true;
  return false;
}

/** Global shortcuts wired once per session at the AppShell.
 *
 *  Conventions:
 *  - Space: toggle play / pause
 *  - ← / → : prev / next track
 *  - L : cycle ambient loop mode
 *  - / : focus the library search box
 *  - 1–8 : switch tabs in tab order (Player, Library, Metadata, Playlists,
 *          Modes, Presets, Controls, Settings)
 *  - Esc : already handled per-modal
 *
 *  All shortcuts no-op when the user is typing (input, textarea, etc.) so
 *  Space in the search box still types a space.
 */
export function useKeyboardShortcuts(): void {
  const navigate = useNavigate();

  useEffect(() => {
    function onKey(e: KeyboardEvent) {
      if (isTypingTarget(e.target)) return;
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
        case "1":
          e.preventDefault();
          navigate("/");
          return;
        case "2":
          e.preventDefault();
          navigate("/library");
          return;
        case "3":
          e.preventDefault();
          navigate("/metadata");
          return;
        case "4":
          e.preventDefault();
          navigate("/playlists");
          return;
        case "5":
          e.preventDefault();
          navigate("/modes");
          return;
        case "6":
          e.preventDefault();
          navigate("/presets");
          return;
        case "7":
          e.preventDefault();
          navigate("/controls");
          return;
        case "8":
          e.preventDefault();
          navigate("/settings");
          return;
      }
    }

    window.addEventListener("keydown", onKey);
    return () => window.removeEventListener("keydown", onKey);
  }, [navigate]);
}
