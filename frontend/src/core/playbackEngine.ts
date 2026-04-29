/**
 * Plain `<audio>` playback engine driven by `PlayerState`.
 *
 * Channels (each is a plain HTMLAudioElement; no Web Audio):
 *
 *   ambient A  ┐
 *   ambient B  ┴── two elements so we can crossfade by overlapping playback
 *   interrupt  ── short overrides; pauses ambient while live, resumes after
 *   sfx (transient) ── one disposable element per fire_sfx event
 *
 * Volume is the product of master × per-channel gain, applied directly to
 * each element's `.volume`. Crossfade and interrupt fade-in/out animate
 * the per-channel gain via requestAnimationFrame, so we never have to
 * touch a Web Audio graph.
 *
 * Why not Web Audio? Wrapping an `<audio>` in `MediaElementAudioSourceNode`
 * removes the element's default output and routes it through the
 * AudioContext. If the context is `suspended` (browser autoplay policy)
 * `currentTime` still advances but nothing reaches the speakers — exactly
 * the silent-but-ticking failure mode we hit. Plain `<audio>` only needs
 * a single user gesture before its first `play()` and after that just
 * works. Preset effects (which *do* need Web Audio) are deferred — see
 * `docs/FUTURE.md`.
 */

import { toast } from "@/core/toast";
import type { PlayerState } from "@/core/types";

export interface EffectSpec {
  type: string;
  [key: string]: unknown;
}

export interface PresetManifest {
  id: string;
  name: string;
  description?: string | null;
  effects: EffectSpec[];
}

interface AmbientChannel {
  el: HTMLAudioElement;
  /** Per-channel gain (0..1) used for crossfade. Multiplied by master to
   *  produce the actual `audio.volume`. */
  gain: number;
  /** A monotonically-incrementing token so a stale animation frame
   *  callback can tell it's been superseded by a newer animation. */
  rampToken: number;
}

type Slot = "A" | "B";

const POSITION_REPORT_INTERVAL_MS = 1000;

function clamp01(v: number): number {
  return Math.max(0, Math.min(1, v));
}

// One-shot guard for the autoplay-blocked toast: the browser blocks every
// programmatic play() until first user gesture, and we don't want to spam
// the toast layer with five identical errors back-to-back.
let autoplayBlockedReported = false;

function attachErrorLogger(label: string, el: HTMLAudioElement): void {
  el.addEventListener("error", () => {
    const err = el.error;
    if (err === null) return;
    // MediaError codes: 1 ABORTED, 2 NETWORK, 3 DECODE, 4 SRC_NOT_SUPPORTED.
    const codeName = ["?", "ABORTED", "NETWORK", "DECODE", "SRC_NOT_SUPPORTED"][err.code] ?? "?";
    console.warn(`[playbackEngine] ${label} audio error`, {
      code: err.code,
      codeName,
      message: err.message,
      src: el.src,
    });
    toast.error(
      `Audio failed to load (${codeName})`,
      err.message || `${label} channel — check the file or the network`,
    );
  });
}

/** Attempt to play an element. Surfaces autoplay-blocked and other failures
 *  via console + toast. Safe to call repeatedly; play() on an
 *  already-playing element resolves immediately. */
function safePlay(label: string, el: HTMLAudioElement): void {
  if (!el.src) return; // nothing to play yet
  void el
    .play()
    .then(() => {
      console.info(`[playbackEngine] ${label} play OK`, {
        src: el.src,
        volume: el.volume,
      });
    })
    .catch((err: DOMException) => {
      console.warn(`[playbackEngine] ${label} play rejected`, {
        name: err.name,
        message: err.message,
        src: el.src,
        volume: el.volume,
        muted: el.muted,
        readyState: el.readyState,
      });
      if (err.name === "NotAllowedError") {
        if (!autoplayBlockedReported) {
          autoplayBlockedReported = true;
          toast.error(
            "Browser blocked playback",
            "Click anywhere on the page once, then press Play again.",
          );
        }
      } else if (err.name !== "AbortError") {
        // AbortError just means we paused mid-load — not interesting.
        toast.error(`Playback failed (${err.name})`, err.message);
      }
    });
}

export class PlaybackEngine {
  private ambientA: AmbientChannel | null = null;
  private ambientB: AmbientChannel | null = null;
  private currentSlot: Slot = "A";

  private interruptEl: HTMLAudioElement | null = null;
  private interruptGain = 0;
  private interruptRampToken = 0;

  // Last applied state — used to skip work when a state_changed broadcast
  // doesn't affect this channel.
  private lastAmbientId: number | null = null;
  private lastInterruptId: number | null = null;
  private lastIsPlaying = false;
  private lastVolume = 1.0;
  private lastInterruptFadeOut = 0;
  private warnedAboutPresets = false;

  private isMyOutput = false;

  private crossfadeMs = 0;
  private crossfadeType: "linear" | "equal_power" | "cut" = "linear";

  private reportTimer: number | null = null;

  private onSkipNext: (() => void) | null = null;
  private onInterruptSkipNext: (() => void) | null = null;
  private onPositionReport: ((ms: number) => void) | null = null;

  // ----- wiring -------------------------------------------------------

  setAmbientElements(a: HTMLAudioElement, b: HTMLAudioElement): void {
    this.ambientA = { el: a, gain: 1, rampToken: 0 };
    this.ambientB = { el: b, gain: 0, rampToken: 0 };
    a.addEventListener("ended", this.handleAmbientEnded);
    b.addEventListener("ended", this.handleAmbientEnded);
    attachErrorLogger("ambientA", a);
    attachErrorLogger("ambientB", b);
    this.applyAmbientVolume(this.ambientA);
    this.applyAmbientVolume(this.ambientB);
  }

  setInterruptElement(el: HTMLAudioElement): void {
    this.interruptEl = el;
    this.interruptGain = 0;
    el.volume = 0;
    el.addEventListener("ended", this.handleInterruptEnded);
    attachErrorLogger("interrupt", el);
  }

  setHandlers(handlers: {
    onSkipNext: () => void;
    onInterruptSkipNext: () => void;
    onPositionReport: (ms: number) => void;
  }): void {
    this.onSkipNext = handlers.onSkipNext;
    this.onInterruptSkipNext = handlers.onInterruptSkipNext;
    this.onPositionReport = handlers.onPositionReport;
  }

  destroy(): void {
    if (this.reportTimer !== null) window.clearInterval(this.reportTimer);
    this.reportTimer = null;
    if (this.ambientA) this.ambientA.el.removeEventListener("ended", this.handleAmbientEnded);
    if (this.ambientB) this.ambientB.el.removeEventListener("ended", this.handleAmbientEnded);
    if (this.interruptEl) this.interruptEl.removeEventListener("ended", this.handleInterruptEnded);
  }

  /** Kept for API compatibility. With plain `<audio>` we don't need an
   *  AudioContext to unlock; the elements' own autoplay policy handles
   *  itself once the user has clicked anywhere and we then call play(). */
  unlock(): void {
    /* intentionally empty */
  }

  setPresets(_presets: PresetManifest[]): void {
    /* No-op in this engine version. Preset effects (Web Audio chain)
     * are deferred — see docs/FUTURE.md. We keep the signature so the
     * rest of the app doesn't have to know. */
  }

  // ----- main state apply ---------------------------------------------

  applyState(state: PlayerState, isMyOutput: boolean): void {
    this.isMyOutput = isMyOutput;
    this.crossfadeMs = state.crossfade_ms ?? 0;
    const t = state.crossfade_type;
    if (t === "linear" || t === "equal_power" || t === "cut") this.crossfadeType = t;

    if (state.volume !== this.lastVolume) {
      this.lastVolume = state.volume;
      this.applyAllVolumes();
    }

    if (state.active_preset_ids.length > 0 && !this.warnedAboutPresets) {
      this.warnedAboutPresets = true;
      // Single warning per session — surfacing it via the toast layer
      // would require a back-channel; console is enough for now.
      console.warn(
        "[playbackEngine] active presets are ignored — effect chain " +
          "(Web Audio) is deferred. See docs/FUTURE.md.",
      );
    }

    if (!isMyOutput) {
      this.silenceAll();
      this.lastAmbientId = null;
      this.lastInterruptId = null;
      this.lastIsPlaying = false;
      return;
    }

    const newAmbientId = state.ambient.current_track_id ?? null;
    const newInterruptId = state.interrupt?.current_track_id ?? null;
    const newIsPlaying = state.is_playing;

    if (state.interrupt) {
      this.lastInterruptFadeOut = state.interrupt.fade_out_ms ?? 0;
    }

    // Interrupt transitions take precedence — they pause/resume ambient.
    if (newInterruptId !== this.lastInterruptId) {
      if (newInterruptId !== null) {
        const fadeIn = state.interrupt?.fade_in_ms ?? 0;
        this.startInterrupt(newInterruptId, fadeIn);
        this.pauseAmbient();
      } else if (this.lastInterruptFadeOut > 0) {
        this.fadeOutInterrupt(this.lastInterruptFadeOut);
      } else {
        this.stopInterrupt();
      }
      this.lastInterruptId = newInterruptId;
    }

    if (newInterruptId === null) {
      if (newAmbientId !== this.lastAmbientId) {
        if (newAmbientId === null) {
          this.stopAmbient();
        } else {
          const useCrossfade =
            this.lastAmbientId !== null && newIsPlaying && this.crossfadeMs > 0;
          this.swapAmbient(newAmbientId, useCrossfade ? this.crossfadeMs : 0);
        }
        this.lastAmbientId = newAmbientId;
      }

      if (newIsPlaying !== this.lastIsPlaying) {
        if (newIsPlaying) this.resumeAmbient();
        else this.pauseAmbient();
      }
    }

    this.lastIsPlaying = newIsPlaying;
    this.scheduleReports();
  }

  // ----- SFX ---------------------------------------------------------

  fireSfx(streamUrl: string, volume: number): void {
    if (!this.isMyOutput) return;
    const el = new Audio();
    el.src = streamUrl;
    el.volume = clamp01(volume) * clamp01(this.lastVolume);
    attachErrorLogger("sfx", el);
    const cleanup = (): void => {
      el.removeEventListener("ended", cleanup);
    };
    el.addEventListener("ended", cleanup);
    safePlay("sfx", el);
  }

  // ----- volume math --------------------------------------------------

  private applyAmbientVolume(ch: AmbientChannel): void {
    ch.el.volume = clamp01(this.lastVolume) * clamp01(ch.gain);
  }

  private applyInterruptVolume(): void {
    if (this.interruptEl) {
      this.interruptEl.volume = clamp01(this.lastVolume) * clamp01(this.interruptGain);
    }
  }

  private applyAllVolumes(): void {
    if (this.ambientA) this.applyAmbientVolume(this.ambientA);
    if (this.ambientB) this.applyAmbientVolume(this.ambientB);
    this.applyInterruptVolume();
  }

  // ----- ambient ------------------------------------------------------

  private currentChannel(): AmbientChannel | null {
    return this.currentSlot === "A" ? this.ambientA : this.ambientB;
  }
  private otherChannel(): AmbientChannel | null {
    return this.currentSlot === "A" ? this.ambientB : this.ambientA;
  }

  private loadInto(ch: AmbientChannel, streamUrl: string): void {
    ch.el.pause();
    if (!ch.el.src.endsWith(streamUrl)) {
      ch.el.src = streamUrl;
      ch.el.load();
    }
  }

  private swapAmbient(trackId: number, crossfadeMs: number): void {
    const url = `/api/library/tracks/${trackId}/stream`;
    const current = this.currentChannel();
    const other = this.otherChannel();
    if (!current || !other) return;

    if (crossfadeMs <= 0) {
      // Snap-cut on the current channel.
      this.loadInto(current, url);
      this.setGainNow(current, 1);
      this.setGainNow(other, 0);
      if (this.lastIsPlaying) safePlay("ambient", current.el);
      return;
    }

    // Crossfade: load on the OTHER channel and animate gains. After the
    // ramp finishes, the other channel becomes "current".
    this.loadInto(other, url);
    safePlay("ambient (incoming)", other.el);
    this.rampGain(current, current.gain, 0, crossfadeMs, () => {
      current.el.pause();
    });
    this.rampGain(other, 0, 1, crossfadeMs);
    this.currentSlot = this.currentSlot === "A" ? "B" : "A";
  }

  private resumeAmbient(): void {
    if (this.lastInterruptId !== null) return;
    const ch = this.currentChannel();
    if (ch) safePlay("ambient", ch.el);
  }

  private pauseAmbient(): void {
    this.ambientA?.el.pause();
    this.ambientB?.el.pause();
  }

  private stopAmbient(): void {
    this.pauseAmbient();
    if (this.ambientA?.el) this.ambientA.el.removeAttribute("src");
    if (this.ambientB?.el) this.ambientB.el.removeAttribute("src");
    this.ambientA?.el.load();
    this.ambientB?.el.load();
  }

  private setGainNow(ch: AmbientChannel, value: number): void {
    ch.rampToken += 1; // cancel any in-flight ramp on this channel
    ch.gain = clamp01(value);
    this.applyAmbientVolume(ch);
  }

  /** Animate a channel's gain from `from` to `to` over `ms`, applying
   *  the curve set on `crossfadeType`. Calls `onDone` when complete
   *  (unless cancelled by another ramp on the same channel). */
  private rampGain(
    ch: AmbientChannel,
    from: number,
    to: number,
    ms: number,
    onDone?: () => void,
  ): void {
    ch.rampToken += 1;
    const myToken = ch.rampToken;
    ch.gain = clamp01(from);
    this.applyAmbientVolume(ch);
    if (ms <= 0) {
      ch.gain = clamp01(to);
      this.applyAmbientVolume(ch);
      onDone?.();
      return;
    }
    const start = performance.now();
    const tick = (now: number) => {
      if (ch.rampToken !== myToken) return; // superseded
      const t = Math.min(1, (now - start) / ms);
      const eased = this.ease(t);
      ch.gain = clamp01(from + (to - from) * eased);
      this.applyAmbientVolume(ch);
      if (t < 1) {
        window.requestAnimationFrame(tick);
      } else {
        onDone?.();
      }
    };
    window.requestAnimationFrame(tick);
  }

  private ease(t: number): number {
    // `cut` is an instant transition; we already snap-cut for crossfadeMs<=0
    // but if the operator picks "cut" with a non-zero ms, honour the spirit.
    if (this.crossfadeType === "cut") return t < 1 ? 0 : 1;
    if (this.crossfadeType === "equal_power") {
      // Equal-power crossfade: sin/cos of a quarter circle. Smoother than
      // a linear ramp at the centre.
      return Math.sin((t * Math.PI) / 2);
    }
    return t;
  }

  // ----- interrupt ----------------------------------------------------

  private startInterrupt(trackId: number, fadeInMs: number): void {
    if (!this.interruptEl) return;
    const url = `/api/library/tracks/${trackId}/stream`;
    if (!this.interruptEl.src.endsWith(url)) {
      this.interruptEl.src = url;
      this.interruptEl.load();
    }
    this.rampInterrupt(0, 1, fadeInMs);
    safePlay("interrupt", this.interruptEl);
  }

  private fadeOutInterrupt(ms: number): void {
    this.rampInterrupt(this.interruptGain, 0, ms, () => this.stopInterrupt());
  }

  private stopInterrupt(): void {
    if (this.interruptEl) {
      this.interruptEl.pause();
      this.interruptEl.removeAttribute("src");
      this.interruptEl.load();
    }
    this.interruptGain = 0;
    this.applyInterruptVolume();
    if (this.lastIsPlaying) {
      const ch = this.currentChannel();
      if (ch) safePlay("ambient", ch.el);
    }
  }

  private rampInterrupt(
    from: number,
    to: number,
    ms: number,
    onDone?: () => void,
  ): void {
    this.interruptRampToken += 1;
    const myToken = this.interruptRampToken;
    this.interruptGain = clamp01(from);
    this.applyInterruptVolume();
    if (ms <= 0) {
      this.interruptGain = clamp01(to);
      this.applyInterruptVolume();
      onDone?.();
      return;
    }
    const start = performance.now();
    const tick = (now: number) => {
      if (this.interruptRampToken !== myToken) return;
      const t = Math.min(1, (now - start) / ms);
      this.interruptGain = clamp01(from + (to - from) * t);
      this.applyInterruptVolume();
      if (t < 1) {
        window.requestAnimationFrame(tick);
      } else {
        onDone?.();
      }
    };
    window.requestAnimationFrame(tick);
  }

  private silenceAll(): void {
    this.pauseAmbient();
    if (this.interruptEl) this.interruptEl.pause();
    this.interruptGain = 0;
    this.applyInterruptVolume();
  }

  // ----- position reports --------------------------------------------

  private scheduleReports(): void {
    if (this.reportTimer !== null) return;
    this.reportTimer = window.setInterval(() => {
      if (!this.isMyOutput || !this.lastIsPlaying || !this.onPositionReport) return;
      const ms = this.currentPositionMs();
      if (Number.isFinite(ms) && ms >= 0) this.onPositionReport(Math.floor(ms));
    }, POSITION_REPORT_INTERVAL_MS);
  }

  private currentPositionMs(): number {
    if (this.lastInterruptId !== null && this.interruptEl) {
      return this.interruptEl.currentTime * 1000;
    }
    const ch = this.currentChannel();
    if (!ch) return 0;
    return ch.el.currentTime * 1000;
  }

  // ----- diagnostics -------------------------------------------------

  /** Snapshot of engine state for the Settings → Diagnostics panel.
   *  Plain JSON; cheap to call. */
  getDiagnostics(): {
    isMyOutput: boolean;
    masterVolume: number;
    currentSlot: Slot;
    lastIsPlaying: boolean;
    lastAmbientId: number | null;
    lastInterruptId: number | null;
    channels: {
      label: string;
      gain: number;
      paused: boolean;
      muted: boolean;
      volume: number;
      readyState: number;
      networkState: number;
      currentTime: number;
      duration: number;
      src: string;
      errorCode: number | null;
    }[];
  } {
    const snap = (label: string, ch: AmbientChannel | null) =>
      ch === null
        ? null
        : {
            label,
            gain: ch.gain,
            paused: ch.el.paused,
            muted: ch.el.muted,
            volume: ch.el.volume,
            readyState: ch.el.readyState,
            networkState: ch.el.networkState,
            currentTime: ch.el.currentTime,
            duration: ch.el.duration,
            src: ch.el.src,
            errorCode: ch.el.error?.code ?? null,
          };
    const interrupt =
      this.interruptEl === null
        ? null
        : {
            label: "interrupt",
            gain: this.interruptGain,
            paused: this.interruptEl.paused,
            muted: this.interruptEl.muted,
            volume: this.interruptEl.volume,
            readyState: this.interruptEl.readyState,
            networkState: this.interruptEl.networkState,
            currentTime: this.interruptEl.currentTime,
            duration: this.interruptEl.duration,
            src: this.interruptEl.src,
            errorCode: this.interruptEl.error?.code ?? null,
          };
    return {
      isMyOutput: this.isMyOutput,
      masterVolume: this.lastVolume,
      currentSlot: this.currentSlot,
      lastIsPlaying: this.lastIsPlaying,
      lastAmbientId: this.lastAmbientId,
      lastInterruptId: this.lastInterruptId,
      channels: [
        snap("ambientA", this.ambientA),
        snap("ambientB", this.ambientB),
        interrupt,
      ].filter((x): x is NonNullable<typeof x> => x !== null),
    };
  }

  // ----- DOM event handlers (bound) ----------------------------------

  private handleAmbientEnded = (e: Event): void => {
    const target = e.currentTarget;
    const cur = this.currentChannel();
    if (cur && target === cur.el) {
      this.onSkipNext?.();
    }
  };

  private handleInterruptEnded = (): void => {
    if (this.lastInterruptFadeOut > 0) {
      this.fadeOutInterrupt(this.lastInterruptFadeOut);
    }
    this.onInterruptSkipNext?.();
  };
}

export const playbackEngine = new PlaybackEngine();
