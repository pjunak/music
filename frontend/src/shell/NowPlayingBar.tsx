import { useEffect, useRef, useState } from "react";
import type { KeyboardEvent, MouseEvent } from "react";

import { IconButton } from "@/components/IconButton";
import {
  InfinityIcon,
  LightningIcon,
  PauseIcon,
  PlayIcon,
  RepeatIcon,
  RepeatOneIcon,
  ShuffleIcon,
  ShuffleWeightedIcon,
  SkipNextIcon,
  SkipPrevIcon,
} from "@/components/icons";
import type { ModeOption } from "@/components/ModeCycleButton";
import { ModeCycleButton } from "@/components/ModeCycleButton";
import { SpeakersControl } from "@/components/SpeakersControl";
import { VolumeControl } from "@/components/VolumeControl";
import { libraryApi } from "@/core/api";
import { useAuthStore } from "@/core/auth";
import {
  selectActivePositionMs,
  usePlayerStore,
} from "@/core/playerStore";
import { trackTitle } from "@/core/trackDisplay";
import type { LoopMode, ShuffleMode, Track } from "@/core/types";
import { useTickWhile } from "@/core/useTickWhile";
import { wsClient } from "@/core/ws";

function formatTime(ms: number): string {
  if (!Number.isFinite(ms) || ms < 0) return "0:00";
  const totalSec = Math.floor(ms / 1000);
  const min = Math.floor(totalSec / 60);
  const sec = totalSec % 60;
  return `${min}:${sec.toString().padStart(2, "0")}`;
}

// Repeat cycle: off → single (current track) → whole queue. Mirrors the
// `L` keyboard shortcut's order.
const REPEAT_OPTIONS: ModeOption<LoopMode>[] = [
  {
    value: "off",
    icon: <RepeatIcon />,
    active: false,
    legend: "Repeat off — click to repeat the current track",
  },
  {
    value: "track",
    icon: <RepeatOneIcon />,
    active: true,
    legend: "Repeating current track — click to repeat the whole queue",
  },
  {
    value: "queue",
    icon: <RepeatIcon />,
    active: true,
    legend: "Repeating the whole queue — click to turn repeat off",
  },
];

// Continue ("Autoplay"): a 2-state toggle (off ↔ follow) that shares the loop
// enum with repeat. Because it reads the same `loop` value, engaging repeat
// shows Continue as off and vice-versa — the two are mutually exclusive for
// free. When loop is "queue"/"track" (not in these options) ModeCycleButton
// falls back to index 0 = off, so it correctly reads as off.
const CONTINUE_OPTIONS: ModeOption<LoopMode>[] = [
  {
    value: "off",
    icon: <InfinityIcon />,
    active: false,
    legend: "Continue off — click to keep playing into the library when the queue ends",
  },
  {
    value: "follow",
    icon: <InfinityIcon />,
    active: true,
    legend: "Continuing into the library when the queue ends — click to turn off",
  },
];

// Shuffle cycle: off → random → weighted. Weighted draws uniformly for now
// (the weighting algorithm is still to be implemented), hence the "planned"
// note in its legend.
const SHUFFLE_OPTIONS: ModeOption<ShuffleMode>[] = [
  {
    value: "off",
    icon: <ShuffleIcon />,
    active: false,
    legend: "Shuffle off — click for random order",
  },
  {
    value: "random",
    icon: <ShuffleIcon />,
    active: true,
    legend: "Shuffle: random — click for weighted random",
  },
  {
    value: "weighted",
    icon: <ShuffleWeightedIcon />,
    active: true,
    legend:
      "Shuffle: weighted random (planned — equal weights for now) — click to turn shuffle off",
  },
];

/** Persistent footer.
 *
 *  Three rows on the inside, but visually a single bar:
 *    1. Track meta (title / artist) on the left, transport buttons centred,
 *       volume on the right.
 *    2. A wide, easy-to-grab scrub bar with elapsed/total clocks bracketing
 *       it. The scrub bar is the headline interactive element here — taller
 *       than the inline volume slider so the two read as different controls.
 *
 *  Since this bar is mounted on every route, every shortcut path through
 *  the app has playback / volume / seek without having to leave the
 *  current view. */
export function NowPlayingBar() {
  const isPlaying = usePlayerStore((s) => s.state?.is_playing ?? false);
  const currentId = usePlayerStore((s) => s.state?.ambient.current_track_id ?? null);
  const interruptId = usePlayerStore(
    (s) => s.state?.interrupt?.current_track_id ?? null,
  );
  const volume = usePlayerStore((s) => s.state?.volume ?? 1);
  const loop = usePlayerStore((s) => s.state?.ambient.loop ?? "off");
  const shuffle = usePlayerStore((s) => s.state?.ambient.shuffle ?? "off");
  const isGuest = useAuthStore((s) => s.status !== "authenticated");

  const [track, setTrack] = useState<Track | null>(null);
  const progressRef = useRef<HTMLDivElement | null>(null);

  // Fetch metadata for whatever's currently playing (interrupt wins).
  const displayId = interruptId ?? currentId;
  const hasTrack = displayId !== null;
  useEffect(() => {
    if (displayId === null) {
      setTrack(null);
      return;
    }
    let cancelled = false;
    void (async () => {
      try {
        const t = await libraryApi.getTrack(displayId);
        if (!cancelled) setTrack(t);
      } catch {
        if (!cancelled) setTrack(null);
      }
    })();
    return () => {
      cancelled = true;
    };
  }, [displayId]);

  // Force re-renders twice a second while playing so the dead-reckoned
  // active-lane position (computed from `stateReceivedAt` in the player
  // store) advances visually without requiring a server-side update.
  // An active interrupt ticks even with is_playing=false (pause only
  // freezes ambient), hence the interruptId term. Gated on hasTrack too:
  // with no track loaded there's nothing to advance, and ticking would
  // just spin the render loop for a clock that should read 0:00.
  useTickWhile((isPlaying || interruptId !== null) && hasTrack, 500);

  const positionMs = usePlayerStore(selectActivePositionMs);
  const totalMs = track !== null ? Math.round(track.length_s * 1000) : 0;

  function play() {
    wsClient.send({ type: "resume" });
  }
  function pause() {
    wsClient.send({ type: "pause" });
  }
  function next() {
    wsClient.send({ type: "ambient_skip_next" });
  }
  function prev() {
    wsClient.send({ type: "ambient_skip_prev" });
  }
  function setRepeat(mode: LoopMode) {
    wsClient.send({ type: "ambient_set_loop", loop: mode });
  }
  function setShuffle(mode: ShuffleMode) {
    wsClient.send({ type: "ambient_set_shuffle", shuffle: mode });
  }

  function seekTo(ms: number) {
    const clamped = Math.max(0, Math.min(totalMs, Math.round(ms)));
    if (interruptId !== null) {
      wsClient.send({ type: "interrupt_seek", position_ms: clamped });
    } else {
      wsClient.send({ type: "ambient_seek", position_ms: clamped });
    }
  }

  function onSeek(e: MouseEvent<HTMLDivElement>) {
    if (totalMs <= 0) return;
    const el = progressRef.current;
    if (el === null) return;
    const rect = el.getBoundingClientRect();
    const fraction = Math.max(0, Math.min(1, (e.clientX - rect.left) / rect.width));
    seekTo(fraction * totalMs);
  }

  /** Keyboard handler for the seek bar when it has focus. Mirrors the
   *  HTML5 range-slider conventions:
   *    ←/→         : ±5s
   *    Shift+←/→   : ±30s
   *    Home / End  : jump to start / end
   *
   *  Important: e.preventDefault() blocks the global ←/→ shortcut from
   *  ALSO firing prev/next, but only for React's synthetic dispatch.
   *  The native window-level keydown listener in useKeyboardShortcuts is
   *  separately guarded by the role="slider" check in isInteractiveTarget. */
  function onSeekKey(e: KeyboardEvent<HTMLDivElement>) {
    if (totalMs <= 0) return;
    const big = e.shiftKey ? 30000 : 5000;
    switch (e.key) {
      case "ArrowLeft":
        e.preventDefault();
        seekTo(positionMs - big);
        return;
      case "ArrowRight":
        e.preventDefault();
        seekTo(positionMs + big);
        return;
      case "Home":
        e.preventDefault();
        seekTo(0);
        return;
      case "End":
        e.preventDefault();
        seekTo(totalMs);
        return;
    }
  }

  function onVolumeChange(next: number) {
    wsClient.send({ type: "set_volume", volume: next });
  }

  const seekable = totalMs > 0;
  const fraction = seekable ? Math.min(1, positionMs / totalMs) : 0;
  // `hasTrack` (computed above) disables transport when nothing's loaded —
  // otherwise pressing play sets is_playing=true server-side and the
  // position counter starts ticking against an empty track, looking like
  // playback when nothing's actually queued.

  return (
    <footer className="now-playing">
      <div className="now-playing-top">
        <div className="now-playing-track">
          {track !== null ? (
            <>
              <strong>{trackTitle(track) || "(untitled)"}</strong>
              <span className="muted small now-playing-track-meta">
                {track.artist || "(unknown)"}
                {track.album ? ` · ${track.album}` : ""}
                {interruptId !== null ? (
                  <span className="now-playing-track-interrupt">
                    {" · "}
                    <LightningIcon />
                    {" interrupt"}
                  </span>
                ) : null}
              </span>
            </>
          ) : (
            <span className="muted">Nothing playing</span>
          )}
        </div>

        <div className="now-playing-controls">
          <ModeCycleButton
            options={SHUFFLE_OPTIONS}
            current={shuffle}
            onCycle={setShuffle}
            readOnly={isGuest}
            readOnlyHint="sign in to change"
          />
          <IconButton
            label="Previous (←)"
            icon={<SkipPrevIcon />}
            onClick={prev}
            disabled={!hasTrack}
          />
          {isPlaying ? (
            <IconButton
              label="Pause (Space)"
              icon={<PauseIcon />}
              onClick={pause}
              className="now-playing-play"
              disabled={!hasTrack}
            />
          ) : (
            <IconButton
              label="Play (Space)"
              icon={<PlayIcon />}
              onClick={play}
              className="now-playing-play"
              disabled={!hasTrack}
            />
          )}
          <IconButton
            label="Next (→)"
            icon={<SkipNextIcon />}
            onClick={next}
            disabled={!hasTrack}
          />
          <ModeCycleButton
            options={REPEAT_OPTIONS}
            current={loop}
            onCycle={setRepeat}
            readOnly={isGuest}
            readOnlyHint="sign in to change"
          />
          <ModeCycleButton
            options={CONTINUE_OPTIONS}
            current={loop}
            onCycle={setRepeat}
            readOnly={isGuest}
            readOnlyHint="sign in to change"
          />
        </div>

        <div className="now-playing-right">
          <SpeakersControl />
          <VolumeControl
            value={volume}
            onChange={onVolumeChange}
            label="Master volume"
            className="now-playing-volume"
            readOnly={isGuest}
            readOnlyTitle="Master volume — sign in to change"
          />
        </div>
      </div>

      <div className="now-playing-scrub">
        <span className="now-playing-clock">{formatTime(positionMs)}</span>
        <div
          ref={progressRef}
          className={`seek-bar seek-bar-large${seekable ? "" : " seek-bar-disabled"}`}
          onClick={seekable ? onSeek : undefined}
          onKeyDown={seekable ? onSeekKey : undefined}
          role={seekable ? "slider" : undefined}
          aria-label={seekable ? "Seek" : undefined}
          aria-valuemin={0}
          aria-valuemax={totalMs}
          aria-valuenow={Math.min(positionMs, totalMs)}
          aria-valuetext={`${formatTime(positionMs)} of ${formatTime(totalMs)}`}
          tabIndex={seekable ? 0 : -1}
          title={seekable ? "Click to seek; arrow keys when focused (Shift = 30s)" : undefined}
        >
          <div
            className="seek-bar-fill"
            style={{ width: `${(fraction * 100).toFixed(2)}%` }}
          />
          <div
            className="seek-bar-thumb"
            style={{ left: `${(fraction * 100).toFixed(2)}%` }}
            aria-hidden="true"
          />
        </div>
        <span className="now-playing-clock">
          {seekable ? formatTime(totalMs) : "—"}
        </span>
      </div>
    </footer>
  );
}
