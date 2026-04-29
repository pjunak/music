import { useEffect, useRef } from "react";

import { playbackEngine } from "@/core/playbackEngine";
import { selectIsMyOutput, usePlayerStore } from "@/core/playerStore";
import { wsClient } from "@/core/ws";

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
      onPositionReport: (ms) =>
        wsClient.send({ type: "position_report", position_ms: ms }),
    });
    return () => {
      playbackEngine.destroy();
    };
  }, []);

  // Drive the engine from PlayerState.
  useEffect(() => {
    const unsub = usePlayerStore.subscribe((s) => {
      if (s.state === null) return;
      const isMine = selectIsMyOutput(s);
      playbackEngine.applyState(s.state, isMine);
    });
    return unsub;
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

  // Auto-claim active output: first state snapshot we see with no active
  // outputs and our device having `audio_output` capability triggers a
  // self-claim, so playing actually produces audio without the operator
  // having to dive into Controls.
  useEffect(() => {
    let claimed = false;
    const unsub = usePlayerStore.subscribe((s) => {
      if (claimed || s.state === null || s.myDeviceId === null) return;
      const me = s.state.connected_devices.find(
        (d) => d.device_id === s.myDeviceId,
      );
      if (!me || !me.capabilities.includes("audio_output")) return;
      if (s.state.active_output_device_ids.length > 0) {
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
