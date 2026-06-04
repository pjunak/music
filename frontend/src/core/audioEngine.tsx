import { useEffect, useRef } from "react";

import { presetsApi } from "@/core/api";
import type { PresetManifest } from "@/core/api";
import { playbackEngine } from "@/core/playbackEngine";
import { selectIsMyOutput, usePlayerStore } from "@/core/playerStore";
import { toast } from "@/core/toast";
import { useUiStore } from "@/core/uiStore";
import { wsClient } from "@/core/ws";


/** True when this device should produce audio locally — either it's in the
 *  server's active outputs, or the user flipped the local override (guest
 *  fallback). */
function isThisDevicePlaying(): boolean {
  const player = usePlayerStore.getState();
  if (selectIsMyOutput(player)) return true;
  return useUiStore.getState().forceLocalPlayback;
}

/** Mounts the three hidden `<audio>` elements (two ambient for crossfade,
 *  one interrupt) and forwards player state + WS events to the engine.
 *  Renders no visible UI of its own. */
export function AudioEngine() {
  const ambientARef = useRef<HTMLAudioElement | null>(null);
  const ambientBRef = useRef<HTMLAudioElement | null>(null);
  const interruptRef = useRef<HTMLAudioElement | null>(null);

  // Wiring: connect DOM refs to the engine and register handlers.
  useEffect(() => {
    if (!ambientARef.current || !ambientBRef.current || !interruptRef.current) return;
    playbackEngine.setAmbientElements(ambientARef.current, ambientBRef.current);
    playbackEngine.setInterruptElement(interruptRef.current);
    playbackEngine.setHandlers({
      onSkipNext: () => wsClient.send({ type: "ambient_skip_next" }),
      onInterruptSkipNext: () => wsClient.send({ type: "interrupt_skip_next" }),
      // Returns whether the report reached the server — the engine uses that
      // to gate its seek-detection baseline.
      onPositionReport: (ms) =>
        wsClient.send({ type: "position_report", position_ms: ms }),
    });
    return () => {
      playbackEngine.destroy();
    };
  }, []);

  // First user gesture unlocks the AudioContext. Required before any audio
  // can reach the speakers under modern browser autoplay policies.
  // `once: true` auto-removes after the first fire, so we don't even need
  // a cleanup path for the (very common) "user clicked once and we're
  // done" case. `capture: true` so we beat any handler that calls
  // stopPropagation (defence in depth — none currently does).
  useEffect(() => {
    const onGesture = () => {
      playbackEngine.unlock();
    };
    const opts: AddEventListenerOptions = { once: true, capture: true };
    window.addEventListener("pointerdown", onGesture, opts);
    window.addEventListener("keydown", onGesture, opts);
    return () => {
      // Idempotent if the listener already self-removed via once: true.
      window.removeEventListener("pointerdown", onGesture, opts);
      window.removeEventListener("keydown", onGesture, opts);
    };
  }, []);

  // Drive the engine from PlayerState. We re-apply on either side of the
  // "is this my output" check changing — the server-side flag from
  // PlayerState OR the local force-playback toggle (guest path).
  useEffect(() => {
    const apply = () => {
      const s = usePlayerStore.getState();
      if (s.state === null) return;
      playbackEngine.applyState(s.state, isThisDevicePlaying());
    };
    const unsubPlayer = usePlayerStore.subscribe(apply);
    const unsubUi = useUiStore.subscribe((s, prev) => {
      if (s.forceLocalPlayback !== prev.forceLocalPlayback) apply();
    });
    return () => {
      unsubPlayer();
      unsubUi();
    };
  }, []);

  // Active presets → effect chain. Cache full manifests indexed by id so
  // we don't refetch the entire list each time a preset toggles. Re-resolve
  // on activation events that include unknown ids (e.g. a new preset was
  // created elsewhere mid-session).
  useEffect(() => {
    const cache = new Map<string, PresetManifest>();
    let isMine = false;

    let toastedError = false;
    async function syncPresets(activeIds: string[]): Promise<void> {
      const missing = activeIds.filter((id) => !cache.has(id));
      if (missing.length > 0) {
        try {
          const all = await presetsApi.list();
          cache.clear();
          for (const m of all) cache.set(m.id, m);
          toastedError = false;
        } catch (err) {
          // Surface once per consecutive failure run so the operator
          // notices that the active preset chain stopped tracking
          // server changes (otherwise this only ended up in console).
          if (!toastedError) {
            toast.error(
              "Failed to load EQ presets",
              err instanceof Error ? err.message : undefined,
            );
            toastedError = true;
          }
          console.warn("[AudioEngine] failed to load presets", err);
        }
      }
      const active = activeIds
        .map((id) => cache.get(id))
        .filter((m): m is PresetManifest => m !== undefined);
      // Only push the chain to the engine on the device that actually plays
      // — non-output clients have no AudioContext yet and don't need it.
      if (isMine) playbackEngine.setPresets(active);
    }

    let lastSig = "";
    const recompute = () => {
      const s = usePlayerStore.getState();
      if (s.state === null) return;
      isMine = isThisDevicePlaying();
      const ids = s.state.active_preset_ids;
      const sig = `${isMine ? "1" : "0"}|${ids.join(",")}`;
      if (sig === lastSig) return;
      lastSig = sig;
      void syncPresets(ids);
    };
    const unsubPlayer = usePlayerStore.subscribe(recompute);
    const unsubUi = useUiStore.subscribe((s, prev) => {
      if (s.forceLocalPlayback !== prev.forceLocalPlayback) recompute();
    });
    return () => {
      unsubPlayer();
      unsubUi();
    };
  }, []);

  // SFX: fire-and-forget transient audio elements per `sfx_fired` event.
  useEffect(() => {
    const unsub = wsClient.subscribe((msg) => {
      if (msg.type === "sfx_fired") {
        const url = `/api/sfx/file?path=${encodeURIComponent(msg.item_path)}`;
        playbackEngine.fireSfx(url, msg.volume);
      }
    });
    return unsub;
  }, []);

  // NB: there is intentionally NO auto-claim of the active-output slot. Output
  // is fully manual — a device becomes a speaker only when the operator
  // designates it (Settings → Devices) and activates it (footer / Console
  // picker). This is what stops a signed-in tab from spontaneously playing
  // audio out loud on refresh. See `app.devices.store` + OutputToggle.

  return (
    <>
      <audio ref={ambientARef} hidden preload="auto" />
      <audio ref={ambientBRef} hidden preload="auto" />
      <audio ref={interruptRef} hidden preload="auto" />
    </>
  );
}
