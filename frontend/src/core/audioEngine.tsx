import { useEffect, useRef } from "react";

import { presetsApi } from "@/core/api";
import { playbackEngine } from "@/core/playbackEngine";
import type { PresetManifest } from "@/core/playbackEngine";
import { selectIsMyOutput, usePlayerStore } from "@/core/playerStore";
import { wsClient } from "@/core/ws";

/** Drives the imperative PlaybackEngine from React state.
 *
 *  Renders three hidden <audio> elements (two ambient for crossfade, one
 *  interrupt) plus zero-or-more transient SFX elements created by the
 *  engine itself. No visible UI of its own.
 */
export function AudioEngine() {
  const ambientARef = useRef<HTMLAudioElement | null>(null);
  const ambientBRef = useRef<HTMLAudioElement | null>(null);
  const interruptRef = useRef<HTMLAudioElement | null>(null);

  // --- engine wiring on mount -----------------------------------------

  useEffect(() => {
    if (!ambientARef.current || !ambientBRef.current || !interruptRef.current) return;
    playbackEngine.setAmbientElements(ambientARef.current, ambientBRef.current);
    playbackEngine.setInterruptElement(interruptRef.current);
    playbackEngine.setHandlers({
      onSkipNext: () => wsClient.send({ type: "ambient_skip_next" }),
      onInterruptSkipNext: () => wsClient.send({ type: "interrupt_skip_next" }),
      onPositionReport: (ms) =>
        wsClient.send({ type: "position_report", position_ms: ms }),
    });

    // Browser autoplay policy: AudioContext can't run until a user
    // gesture. Hook a one-shot global click to unlock it.
    const onUserGesture = () => playbackEngine.unlock();
    window.addEventListener("click", onUserGesture, { once: true });
    window.addEventListener("keydown", onUserGesture, { once: true });

    return () => {
      window.removeEventListener("click", onUserGesture);
      window.removeEventListener("keydown", onUserGesture);
      playbackEngine.destroy();
    };
  }, []);

  // --- preset definitions: fetch once, refresh when active set changes --

  useEffect(() => {
    let cancelled = false;
    void presetsApi
      .list()
      .then((presets: PresetManifest[]) => {
        if (!cancelled) playbackEngine.setPresets(presets);
      })
      .catch(() => {
        /* empty preset library is a valid state */
      });
    return () => {
      cancelled = true;
    };
  }, []);

  // --- drive the engine from PlayerState ------------------------------

  useEffect(() => {
    const unsub = usePlayerStore.subscribe((s) => {
      if (s.state === null) return;
      const isMine = selectIsMyOutput(s);
      playbackEngine.applyState(s.state, isMine);
    });
    return unsub;
  }, []);

  // --- WS event subscription: SFX --------------------------------------

  useEffect(() => {
    const unsub = wsClient.subscribe((msg) => {
      if (msg.type === "sfx_fired") {
        const url = `/api/sfx/file?path=${encodeURIComponent(msg.item_path)}`;
        playbackEngine.fireSfx(url, msg.volume);
      }
    });
    return unsub;
  }, []);

  // --- auto-claim active output ---------------------------------------
  //
  // First time we see a state snapshot with no active outputs and our
  // device is registered (id known), claim ourselves so Play actually
  // produces audio. Skip if outputs are already configured — don't fight
  // the user's prior choice.

  useEffect(() => {
    let claimed = false;
    const unsub = usePlayerStore.subscribe((s) => {
      if (claimed || s.state === null || s.myDeviceId === null) return;
      const me = s.state.connected_devices.find(
        (d) => d.device_id === s.myDeviceId,
      );
      if (!me || !me.capabilities.includes("audio_output")) return;
      if (s.state.active_output_device_ids.length > 0) {
        // Someone (maybe us, on a previous run) already configured outputs.
        // If we're already in the set, treat as claimed.
        if (s.state.active_output_device_ids.includes(s.myDeviceId)) claimed = true;
        return;
      }
      claimed = true;
      wsClient.send({
        type: "set_active_outputs",
        device_ids: [s.myDeviceId],
      });
    });
    return unsub;
  }, []);

  return (
    <>
      <audio ref={ambientARef} hidden preload="auto" />
      <audio ref={ambientBRef} hidden preload="auto" />
      <audio ref={interruptRef} hidden preload="auto" />
    </>
  );
}
