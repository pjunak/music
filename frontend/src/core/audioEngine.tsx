import { useEffect, useRef } from "react";

import { selectIsMyOutput, usePlayerStore } from "@/core/playerStore";
import { wsClient } from "@/core/ws";

const POSITION_REPORT_INTERVAL_MS = 1000;

/** Drives a hidden <audio> element from PlayerState.
 *
 *  Responsibilities:
 *  - Keep audio.src in sync with state.ambient.current_beets_id
 *  - Play / pause based on (is_playing && I am an active output)
 *  - Sync volume
 *  - Report position back to the server every second while playing
 *  - On 'ended', send ambient_skip_next so the queue advances
 *
 *  Renders an invisible <audio> tag; no visible UI of its own.
 */
export function AudioEngine() {
  const audioRef = useRef<HTMLAudioElement | null>(null);

  const state = usePlayerStore((s) => s.state);
  const isMyOutput = usePlayerStore(selectIsMyOutput);

  const ambientCurrentId = state?.ambient.current_beets_id ?? null;
  const isPlaying = state?.is_playing ?? false;
  const volume = state?.volume ?? 1.0;
  const interruptActive = state?.interrupt !== null && state?.interrupt !== undefined;

  // Sync audio src whenever the current ambient track changes.
  useEffect(() => {
    const audio = audioRef.current;
    if (audio === null) return;
    if (ambientCurrentId === null) {
      audio.removeAttribute("src");
      audio.load();
      return;
    }
    const targetSrc = `/api/library/tracks/${ambientCurrentId}/stream`;
    // Compare the resolved URL since browsers normalise it.
    const resolved = new URL(targetSrc, window.location.origin).toString();
    if (audio.src !== resolved) {
      audio.src = targetSrc;
      audio.load();
    }
  }, [ambientCurrentId]);

  // Play / pause based on global is_playing AND this client being an
  // active output. Interrupt lane currently bypassed for first-usable —
  // when implemented, the same audio element will switch its src to the
  // interrupt's track.
  useEffect(() => {
    const audio = audioRef.current;
    if (audio === null) return;
    const shouldPlay =
      isPlaying && isMyOutput && ambientCurrentId !== null && !interruptActive;
    if (shouldPlay) {
      void audio.play().catch(() => {
        // Autoplay was blocked or another play() was already pending.
        // Silent — user will retry by clicking play.
      });
    } else {
      audio.pause();
    }
  }, [isPlaying, isMyOutput, ambientCurrentId, interruptActive]);

  // Sync volume.
  useEffect(() => {
    const audio = audioRef.current;
    if (audio === null) return;
    audio.volume = volume;
  }, [volume]);

  // 'ended' → advance the queue.
  useEffect(() => {
    const audio = audioRef.current;
    if (audio === null) return;
    function onEnded() {
      wsClient.send({ type: "ambient_skip_next" });
    }
    audio.addEventListener("ended", onEnded);
    return () => {
      audio.removeEventListener("ended", onEnded);
    };
  }, []);

  // Report position back to the server periodically while we're an
  // active output and audio is actually playing.
  useEffect(() => {
    if (!isPlaying || !isMyOutput || ambientCurrentId === null) return;
    const interval = window.setInterval(() => {
      const audio = audioRef.current;
      if (audio === null) return;
      const positionMs = Math.floor(audio.currentTime * 1000);
      wsClient.send({ type: "position_report", position_ms: positionMs });
    }, POSITION_REPORT_INTERVAL_MS);
    return () => {
      window.clearInterval(interval);
    };
  }, [isPlaying, isMyOutput, ambientCurrentId]);

  return <audio ref={audioRef} hidden preload="auto" />;
}
