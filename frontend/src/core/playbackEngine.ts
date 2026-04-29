/**
 * Playback engine driven by `PlayerState`.
 *
 * Channels:
 *
 *   ambient A  ┐                     ┌── effect chain ──┐
 *   ambient B  ┴── source nodes ─────┤                  ├── ambient master ── destination
 *                                     └── (passthrough) ─┘
 *
 *   interrupt  ── source ── interrupt master ── destination   (bypasses effects)
 *
 *   sfx        ── transient `<audio>` direct to default output (no AudioContext routing)
 *
 * Ambient audio is routed through a single `AudioContext` graph so active
 * preset effects can be applied. Interrupts and SFX bypass the chain — the
 * intent is "scene effects colour the background music; alerts and stings
 * play clean".
 *
 * The biggest risk with Web Audio is the autoplay-blocked failure mode: an
 * `AudioContext` starts `suspended` until a user gesture, and audio routed
 * through it is silently sunk until `ctx.resume()` resolves. We defend
 * against this by:
 *   1. Calling `unlock()` from every user-driven entry point (transport
 *      buttons, play actions, etc.) which `resume()`s the context.
 *   2. Calling `resume()` lazily inside `safePlay()` before each `play()`.
 *   3. Surfacing a one-time toast if the context is still suspended when
 *      we attempt playback.
 *
 * The effect chain is rebuilt whenever active presets change. Each effect
 * type maps to a small graph of native Web Audio nodes — see `buildEffect`.
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
  /** Per-channel gain node controlling crossfade. */
  gainNode: GainNode;
  /** MediaElementAudioSourceNode wrapping `el`. Browsers permit only one
   *  per element; we create it once and reuse. */
  source: MediaElementAudioSourceNode;
  /** Token used to cancel in-flight rAF ramps when superseded. */
  rampToken: number;
}

interface InterruptChannel {
  el: HTMLAudioElement;
  gainNode: GainNode;
  source: MediaElementAudioSourceNode;
  rampToken: number;
}

interface BuiltEffect {
  /** First node in the effect's graph — receives audio from upstream. */
  input: AudioNode;
  /** Last node in the effect's graph — feeds audio downstream. */
  output: AudioNode;
  /** Optional teardown (stop oscillators, etc.). */
  dispose?: () => void;
}

type Slot = "A" | "B";

const POSITION_REPORT_INTERVAL_MS = 1000;

function clamp01(v: number): number {
  return Math.max(0, Math.min(1, v));
}

function numParam(
  effect: EffectSpec,
  key: string,
  fallback: number,
): number {
  const raw = effect[key];
  if (typeof raw === "number" && Number.isFinite(raw)) return raw;
  if (typeof raw === "string" && raw !== "") {
    const parsed = Number(raw);
    if (Number.isFinite(parsed)) return parsed;
  }
  return fallback;
}

let autoplayBlockedReported = false;

function attachErrorLogger(label: string, el: HTMLAudioElement): void {
  el.addEventListener("error", () => {
    const err = el.error;
    if (err === null) return;
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

export class PlaybackEngine {
  private audioContext: AudioContext | null = null;

  private ambientA: AmbientChannel | null = null;
  private ambientB: AmbientChannel | null = null;
  private currentSlot: Slot = "A";

  private interrupt: InterruptChannel | null = null;

  /** Splitter feeding both ambient channels into the head of the effect
   *  chain. Created once when the AudioContext is built. */
  private effectChainHead: GainNode | null = null;
  /** Master ambient gain node — drives master volume and is the chain's
   *  terminal node before `destination`. */
  private ambientMaster: GainNode | null = null;
  /** Master interrupt gain node — bypasses the effect chain. */
  private interruptMaster: GainNode | null = null;

  /** Currently inserted effect graphs. Disposed when presets change. */
  private installedEffects: BuiltEffect[] = [];

  // --- last-applied state, used to short-circuit no-op state changes -----
  private lastAmbientId: number | null = null;
  private lastInterruptId: number | null = null;
  private lastIsPlaying = false;
  private lastVolume = 1.0;
  private lastInterruptFadeOut = 0;
  private lastPresetSignature = "";

  private isMyOutput = false;

  private crossfadeMs = 0;
  private crossfadeType: "linear" | "equal_power" | "cut" = "linear";

  private reportTimer: number | null = null;

  private onSkipNext: (() => void) | null = null;
  private onInterruptSkipNext: (() => void) | null = null;
  private onPositionReport: ((ms: number) => void) | null = null;

  // ----- wiring -------------------------------------------------------

  setAmbientElements(a: HTMLAudioElement, b: HTMLAudioElement): void {
    const ctx = this.ensureAudioContext();
    if (ctx === null) {
      // Web Audio unavailable — bail; we can't drive ambient without a graph.
      console.warn("[playbackEngine] AudioContext unavailable; ambient muted");
      return;
    }
    // Browsers ignore `audio.volume` once a MediaElementAudioSourceNode is
    // attached on Firefox in some setups; we explicitly drive volume via
    // GainNodes from here on. Keep element volume at 1.0.
    a.volume = 1;
    b.volume = 1;
    const sourceA = ctx.createMediaElementSource(a);
    const sourceB = ctx.createMediaElementSource(b);
    const gainA = ctx.createGain();
    const gainB = ctx.createGain();
    gainA.gain.value = 1;
    gainB.gain.value = 0;
    sourceA.connect(gainA);
    sourceB.connect(gainB);
    if (this.effectChainHead !== null) {
      gainA.connect(this.effectChainHead);
      gainB.connect(this.effectChainHead);
    }
    this.ambientA = { el: a, gainNode: gainA, source: sourceA, rampToken: 0 };
    this.ambientB = { el: b, gainNode: gainB, source: sourceB, rampToken: 0 };
    a.addEventListener("ended", this.handleAmbientEnded);
    b.addEventListener("ended", this.handleAmbientEnded);
    attachErrorLogger("ambientA", a);
    attachErrorLogger("ambientB", b);
  }

  setInterruptElement(el: HTMLAudioElement): void {
    const ctx = this.ensureAudioContext();
    if (ctx === null) return;
    el.volume = 1;
    const source = ctx.createMediaElementSource(el);
    const gainNode = ctx.createGain();
    gainNode.gain.value = 0;
    source.connect(gainNode);
    if (this.interruptMaster !== null) {
      gainNode.connect(this.interruptMaster);
    }
    this.interrupt = { el, gainNode, source, rampToken: 0 };
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
    if (this.interrupt) this.interrupt.el.removeEventListener("ended", this.handleInterruptEnded);
    this.disposeEffects();
  }

  /** Resume the AudioContext. Browsers require a user gesture before audio
   *  reaches the speakers; transport buttons / play actions should call
   *  this before their underlying state mutation. Idempotent. */
  unlock(): void {
    const ctx = this.audioContext;
    if (ctx === null || ctx.state === "running") return;
    void ctx.resume().catch((err: unknown) => {
      console.warn("[playbackEngine] AudioContext resume failed", err);
    });
  }

  // ----- Web Audio graph construction --------------------------------

  private ensureAudioContext(): AudioContext | null {
    if (this.audioContext !== null) return this.audioContext;
    const Ctor = window.AudioContext ?? (window as unknown as { webkitAudioContext?: typeof AudioContext }).webkitAudioContext;
    if (Ctor === undefined) return null;
    const ctx = new Ctor();
    this.audioContext = ctx;
    this.ambientMaster = ctx.createGain();
    this.interruptMaster = ctx.createGain();
    this.effectChainHead = ctx.createGain();
    this.ambientMaster.gain.value = clamp01(this.lastVolume);
    this.interruptMaster.gain.value = clamp01(this.lastVolume);
    this.ambientMaster.connect(ctx.destination);
    this.interruptMaster.connect(ctx.destination);
    // No effects yet — passthrough head → ambient master.
    this.effectChainHead.connect(this.ambientMaster);
    ctx.addEventListener("statechange", () => {
      if (ctx.state === "suspended") {
        console.info("[playbackEngine] AudioContext suspended");
      }
    });
    return ctx;
  }

  // ----- preset / effect chain ---------------------------------------

  setPresets(presets: PresetManifest[]): void {
    const ctx = this.audioContext;
    if (ctx === null || this.effectChainHead === null || this.ambientMaster === null) {
      return; // Web Audio not available or not wired yet — no-op.
    }
    // Cheap fingerprint so we can skip rebuilds when the active presets
    // didn't actually change. setPresets gets called on every state push.
    const signature = JSON.stringify(
      presets.map((p) => ({ id: p.id, effects: p.effects })),
    );
    if (signature === this.lastPresetSignature) return;
    this.lastPresetSignature = signature;

    // Tear down the current chain.
    this.disposeEffects();
    this.effectChainHead.disconnect();
    this.ambientMaster.disconnect();
    this.ambientMaster.connect(ctx.destination);

    // Flatten preset chains in declaration order — preset[0].effects then
    // preset[1].effects, etc. Skip unsupported effects; warn once per type.
    const flat: EffectSpec[] = [];
    for (const preset of presets) {
      for (const eff of preset.effects) {
        flat.push(eff);
      }
    }

    if (flat.length === 0) {
      // Empty chain — passthrough.
      this.effectChainHead.connect(this.ambientMaster);
      return;
    }

    let upstream: AudioNode = this.effectChainHead;
    for (const spec of flat) {
      const built = buildEffect(ctx, spec);
      if (built === null) continue;
      upstream.connect(built.input);
      upstream = built.output;
      this.installedEffects.push(built);
    }
    upstream.connect(this.ambientMaster);
  }

  private disposeEffects(): void {
    for (const eff of this.installedEffects) {
      try {
        eff.input.disconnect();
      } catch {
        /* already disconnected */
      }
      try {
        eff.output.disconnect();
      } catch {
        /* already disconnected */
      }
      eff.dispose?.();
    }
    this.installedEffects = [];
  }

  // ----- main state apply ---------------------------------------------

  applyState(state: PlayerState, isMyOutput: boolean): void {
    this.isMyOutput = isMyOutput;
    this.crossfadeMs = state.crossfade_ms ?? 0;
    const t = state.crossfade_type;
    if (t === "linear" || t === "equal_power" || t === "cut") this.crossfadeType = t;

    if (state.volume !== this.lastVolume) {
      this.lastVolume = state.volume;
      this.applyMasterVolume();
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
    // Transient one-shot: route directly via element.volume so a flood of
    // SFX events doesn't pollute the long-lived effect graph.
    const el = new Audio();
    el.src = streamUrl;
    el.volume = clamp01(volume) * clamp01(this.lastVolume);
    attachErrorLogger("sfx", el);
    el.addEventListener("ended", () => {
      el.removeAttribute("src");
    });
    void el.play().catch((err: DOMException) => {
      if (err.name === "AbortError") return;
      console.warn("[playbackEngine] sfx play rejected", err.message);
    });
  }

  // ----- volume / master gain -----------------------------------------

  private applyMasterVolume(): void {
    const v = clamp01(this.lastVolume);
    if (this.ambientMaster) this.ambientMaster.gain.value = v;
    if (this.interruptMaster) this.interruptMaster.gain.value = v;
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
      this.loadInto(current, url);
      this.setGainNow(current, 1);
      this.setGainNow(other, 0);
      if (this.lastIsPlaying) this.safePlay("ambient", current.el);
      return;
    }

    this.loadInto(other, url);
    this.safePlay("ambient (incoming)", other.el);
    this.rampGain(current, current.gainNode.gain.value, 0, crossfadeMs, () => {
      current.el.pause();
    });
    this.rampGain(other, 0, 1, crossfadeMs);
    this.currentSlot = this.currentSlot === "A" ? "B" : "A";
  }

  private resumeAmbient(): void {
    if (this.lastInterruptId !== null) return;
    const ch = this.currentChannel();
    if (ch) this.safePlay("ambient", ch.el);
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
    ch.rampToken += 1;
    ch.gainNode.gain.value = clamp01(value);
  }

  private rampGain(
    ch: AmbientChannel,
    from: number,
    to: number,
    ms: number,
    onDone?: () => void,
  ): void {
    ch.rampToken += 1;
    const myToken = ch.rampToken;
    ch.gainNode.gain.value = clamp01(from);
    if (ms <= 0) {
      ch.gainNode.gain.value = clamp01(to);
      onDone?.();
      return;
    }
    const start = performance.now();
    const tick = (now: number) => {
      if (ch.rampToken !== myToken) return;
      const t = Math.min(1, (now - start) / ms);
      const eased = this.ease(t);
      ch.gainNode.gain.value = clamp01(from + (to - from) * eased);
      if (t < 1) {
        window.requestAnimationFrame(tick);
      } else {
        onDone?.();
      }
    };
    window.requestAnimationFrame(tick);
  }

  private ease(t: number): number {
    if (this.crossfadeType === "cut") return t < 1 ? 0 : 1;
    if (this.crossfadeType === "equal_power") {
      return Math.sin((t * Math.PI) / 2);
    }
    return t;
  }

  // ----- interrupt ----------------------------------------------------

  private startInterrupt(trackId: number, fadeInMs: number): void {
    if (!this.interrupt) return;
    const url = `/api/library/tracks/${trackId}/stream`;
    if (!this.interrupt.el.src.endsWith(url)) {
      this.interrupt.el.src = url;
      this.interrupt.el.load();
    }
    this.rampInterrupt(0, 1, fadeInMs);
    this.safePlay("interrupt", this.interrupt.el);
  }

  private fadeOutInterrupt(ms: number): void {
    if (!this.interrupt) return;
    const from = this.interrupt.gainNode.gain.value;
    this.rampInterrupt(from, 0, ms, () => this.stopInterrupt());
  }

  private stopInterrupt(): void {
    if (this.interrupt) {
      this.interrupt.el.pause();
      this.interrupt.el.removeAttribute("src");
      this.interrupt.el.load();
      this.interrupt.gainNode.gain.value = 0;
    }
    if (this.lastIsPlaying) {
      const ch = this.currentChannel();
      if (ch) this.safePlay("ambient", ch.el);
    }
  }

  private rampInterrupt(
    from: number,
    to: number,
    ms: number,
    onDone?: () => void,
  ): void {
    if (!this.interrupt) return;
    this.interrupt.rampToken += 1;
    const myToken = this.interrupt.rampToken;
    const node = this.interrupt.gainNode;
    node.gain.value = clamp01(from);
    if (ms <= 0) {
      node.gain.value = clamp01(to);
      onDone?.();
      return;
    }
    const start = performance.now();
    const tick = (now: number) => {
      if (!this.interrupt || this.interrupt.rampToken !== myToken) return;
      const t = Math.min(1, (now - start) / ms);
      node.gain.value = clamp01(from + (to - from) * t);
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
    if (this.interrupt) {
      this.interrupt.el.pause();
      this.interrupt.gainNode.gain.value = 0;
    }
  }

  /** play() wrapper that ensures the AudioContext is running before
   *  attempting playback, surfaces autoplay errors via toast, and is safe
   *  to call repeatedly. */
  private safePlay(label: string, el: HTMLAudioElement): void {
    if (!el.src) return;
    const ctx = this.audioContext;
    const startPlay = () => {
      void el
        .play()
        .then(() => {
          console.info(`[playbackEngine] ${label} play OK`, {
            src: el.src,
          });
        })
        .catch((err: DOMException) => {
          console.warn(`[playbackEngine] ${label} play rejected`, {
            name: err.name,
            message: err.message,
            ctxState: ctx?.state ?? "no-ctx",
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
            toast.error(`Playback failed (${err.name})`, err.message);
          }
        });
    };
    if (ctx !== null && ctx.state !== "running") {
      void ctx
        .resume()
        .catch(() => undefined)
        .finally(startPlay);
    } else {
      startPlay();
    }
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
    if (this.lastInterruptId !== null && this.interrupt) {
      return this.interrupt.el.currentTime * 1000;
    }
    const ch = this.currentChannel();
    if (!ch) return 0;
    return ch.el.currentTime * 1000;
  }

  // ----- diagnostics -------------------------------------------------

  getDiagnostics(): {
    isMyOutput: boolean;
    masterVolume: number;
    currentSlot: Slot;
    lastIsPlaying: boolean;
    lastAmbientId: number | null;
    lastInterruptId: number | null;
    audioContextState: string;
    activeEffectCount: number;
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
    const snapAmbient = (label: string, ch: AmbientChannel | null) =>
      ch === null
        ? null
        : {
            label,
            gain: ch.gainNode.gain.value,
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
      this.interrupt === null
        ? null
        : {
            label: "interrupt",
            gain: this.interrupt.gainNode.gain.value,
            paused: this.interrupt.el.paused,
            muted: this.interrupt.el.muted,
            volume: this.interrupt.el.volume,
            readyState: this.interrupt.el.readyState,
            networkState: this.interrupt.el.networkState,
            currentTime: this.interrupt.el.currentTime,
            duration: this.interrupt.el.duration,
            src: this.interrupt.el.src,
            errorCode: this.interrupt.el.error?.code ?? null,
          };
    return {
      isMyOutput: this.isMyOutput,
      masterVolume: this.lastVolume,
      currentSlot: this.currentSlot,
      lastIsPlaying: this.lastIsPlaying,
      lastAmbientId: this.lastAmbientId,
      lastInterruptId: this.lastInterruptId,
      audioContextState: this.audioContext?.state ?? "none",
      activeEffectCount: this.installedEffects.length,
      channels: [
        snapAmbient("ambientA", this.ambientA),
        snapAmbient("ambientB", this.ambientB),
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

// ----- effect builders -----------------------------------------------

const _warnedUnsupportedEffectTypes = new Set<string>();

function buildEffect(ctx: AudioContext, spec: EffectSpec): BuiltEffect | null {
  switch (spec.type) {
    case "lowpass":
    case "highpass":
    case "bandpass":
      return buildBiquad(ctx, spec, spec.type);
    case "delay":
      return buildDelay(ctx, spec);
    case "distortion":
      return buildDistortion(ctx, spec);
    case "tremolo":
      return buildTremolo(ctx, spec);
    case "reverb":
      return buildReverb(ctx, spec);
    case "pitch_shift":
      // Web Audio has no native pitch shifter and a third-party node would
      // double the bundle. Skip silently — the docs in PresetsView call
      // this out so users aren't surprised.
      return null;
    default:
      if (!_warnedUnsupportedEffectTypes.has(spec.type)) {
        _warnedUnsupportedEffectTypes.add(spec.type);
        console.warn(`[playbackEngine] unsupported effect type '${spec.type}' — skipped`);
      }
      return null;
  }
}

function buildBiquad(
  ctx: AudioContext,
  spec: EffectSpec,
  kind: "lowpass" | "highpass" | "bandpass",
): BuiltEffect {
  const node = ctx.createBiquadFilter();
  node.type = kind;
  node.frequency.value = numParam(spec, "frequency", kind === "lowpass" ? 800 : kind === "highpass" ? 200 : 1000);
  node.Q.value = numParam(spec, "q", 0.7);
  return { input: node, output: node };
}

function buildDelay(ctx: AudioContext, spec: EffectSpec): BuiltEffect {
  const time = Math.max(0, Math.min(numParam(spec, "time", 0.25), 5));
  const feedback = clamp01(numParam(spec, "feedback", 0.3));
  const wet = clamp01(numParam(spec, "wet", 0.4));

  const inputNode = ctx.createGain();
  const outputNode = ctx.createGain();
  const delay = ctx.createDelay(5);
  const fb = ctx.createGain();
  const wetGain = ctx.createGain();
  const dryGain = ctx.createGain();

  delay.delayTime.value = time;
  fb.gain.value = feedback;
  wetGain.gain.value = wet;
  dryGain.gain.value = 1;

  inputNode.connect(dryGain);
  inputNode.connect(delay);
  delay.connect(fb);
  fb.connect(delay);
  delay.connect(wetGain);
  wetGain.connect(outputNode);
  dryGain.connect(outputNode);

  return { input: inputNode, output: outputNode };
}

function buildDistortion(ctx: AudioContext, spec: EffectSpec): BuiltEffect {
  const amount = Math.max(0, numParam(spec, "amount", 50));
  const node = ctx.createWaveShaper();
  // WaveShaperNode.curve narrowed to Float32Array<ArrayBuffer> in newer TS
  // libs. Constructing in-place from an ArrayBuffer keeps the type happy
  // without resorting to a cast.
  const n = 4096;
  const curve = new Float32Array(new ArrayBuffer(n * 4));
  const deg = Math.PI / 180;
  for (let i = 0; i < n; i++) {
    const x = (i * 2) / n - 1;
    curve[i] = ((3 + amount) * x * 20 * deg) / (Math.PI + amount * Math.abs(x));
  }
  node.curve = curve;
  node.oversample = "4x";
  return { input: node, output: node };
}

function buildTremolo(ctx: AudioContext, spec: EffectSpec): BuiltEffect {
  const rate = Math.max(0.01, numParam(spec, "rate", 5));
  const depth = clamp01(numParam(spec, "depth", 0.5));

  const gainNode = ctx.createGain();
  gainNode.gain.value = 1 - depth / 2;

  const lfo = ctx.createOscillator();
  lfo.frequency.value = rate;
  const lfoGain = ctx.createGain();
  lfoGain.gain.value = depth / 2;
  lfo.connect(lfoGain);
  lfoGain.connect(gainNode.gain);
  lfo.start();

  return {
    input: gainNode,
    output: gainNode,
    dispose: () => {
      try {
        lfo.stop();
      } catch {
        /* already stopped */
      }
    },
  };
}

function buildReverb(ctx: AudioContext, spec: EffectSpec): BuiltEffect {
  const decay = Math.max(0.05, numParam(spec, "decay", 2));
  const wet = clamp01(numParam(spec, "wet", 0.4));

  const inputNode = ctx.createGain();
  const outputNode = ctx.createGain();
  const convolver = ctx.createConvolver();
  convolver.buffer = makeReverbIR(ctx, decay);

  const wetGain = ctx.createGain();
  const dryGain = ctx.createGain();
  wetGain.gain.value = wet;
  dryGain.gain.value = 1;

  inputNode.connect(dryGain);
  dryGain.connect(outputNode);
  inputNode.connect(convolver);
  convolver.connect(wetGain);
  wetGain.connect(outputNode);

  return { input: inputNode, output: outputNode };
}

function makeReverbIR(ctx: AudioContext, decay: number): AudioBuffer {
  const sampleRate = ctx.sampleRate;
  const length = Math.max(1, Math.floor(sampleRate * decay));
  const buffer = ctx.createBuffer(2, length, sampleRate);
  for (let ch = 0; ch < 2; ch++) {
    const data = buffer.getChannelData(ch);
    for (let i = 0; i < length; i++) {
      const t = i / length;
      data[i] = (Math.random() * 2 - 1) * Math.pow(1 - t, 2);
    }
  }
  return buffer;
}

export const playbackEngine = new PlaybackEngine();
