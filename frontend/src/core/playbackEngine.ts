/**
 * Imperative playback engine driven by `PlayerState`.
 *
 * The audio graph (rough shape):
 *
 *   ambient A ─┐
 *   ambient B ─┼─► gainA/B ─► effect chain ─► master ─► destination
 *              │
 *   interrupt ─┴─────────────► gainI ──────────────────────────► master
 *
 *   sfx (transient) ──────────► gainS ──────────────────────────► master
 *
 * Two ambient elements let us crossfade by overlapping playback. Effects
 * apply to the ambient channel only — interrupts and SFX are intentionally
 * dry so they sound clear regardless of the active preset.
 *
 * Browsers gate AudioContext behind a user gesture; we lazy-init on first
 * `unlock()` (any click in the AppShell) and resume whenever we kick off
 * playback.
 */

import type { PlayerState } from "@/core/types";

export interface EffectSpec {
  type: string;
  // mutagen-style — we accept any extra keys so effect tuning can evolve
  // without a frontend rebuild.
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
  src: MediaElementAudioSourceNode | null;
  gain: GainNode | null;
}

type Slot = "A" | "B";

const POSITION_REPORT_INTERVAL_MS = 1000;

/** True if the float looks like a meaningful audio time. Used to decide
 *  whether to set currentTime when restoring a saved position. */
function isFiniteNonNeg(n: unknown): n is number {
  return typeof n === "number" && Number.isFinite(n) && n >= 0;
}

function clampGain(v: number): number {
  return Math.max(0, Math.min(1, v));
}

export class PlaybackEngine {
  private ctx: AudioContext | null = null;
  private masterGain: GainNode | null = null;
  private effectInput: GainNode | null = null;   // ambient feeds into this; effect chain output → master
  private effectOutput: GainNode | null = null;  // last node of the chain (also where new chains splice in)

  private ambientA: AmbientChannel | null = null;
  private ambientB: AmbientChannel | null = null;
  private currentSlot: Slot = "A";

  private interruptEl: HTMLAudioElement | null = null;
  private interruptSrc: MediaElementAudioSourceNode | null = null;
  private interruptGain: GainNode | null = null;

  // Loaded preset definitions, keyed by id. Re-fetched from the server on
  // login or on `set_active_presets` for ids we haven't seen yet.
  private presets = new Map<string, PresetManifest>();

  // Track what we last applied so we don't churn the graph on every state
  // broadcast (a state broadcast fires for any field, including unrelated
  // ones like volume).
  private lastAmbientId: number | null = null;
  private lastInterruptId: number | null = null;
  private lastIsPlaying = false;
  private lastVolume = 1.0;
  private lastActivePresetIds: string[] = [];
  private lastInterruptFadeOut = 0;

  // True iff the local browser tab is currently in the active output set.
  private isMyOutput = false;

  // ms — used for crossfade between ambient tracks.
  private crossfadeMs = 0;
  private crossfadeType: "linear" | "equal_power" | "cut" = "linear";

  // Interrupt fade timing for the next-fired interrupt.
  private pendingInterruptFadeIn = 0;
  private pendingInterruptFadeOut = 0;

  // Position-report ticker.
  private reportTimer: number | null = null;

  // Listeners for outgoing actions (so the engine can ask the WS to advance).
  private onSkipNext: (() => void) | null = null;
  private onInterruptSkipNext: (() => void) | null = null;
  private onPositionReport: ((ms: number) => void) | null = null;

  // ----- wiring --------------------------------------------------------

  setAmbientElements(a: HTMLAudioElement, b: HTMLAudioElement): void {
    this.ambientA = { el: a, src: null, gain: null };
    this.ambientB = { el: b, src: null, gain: null };
    a.addEventListener("ended", this.handleAmbientEnded);
    b.addEventListener("ended", this.handleAmbientEnded);
  }

  setInterruptElement(el: HTMLAudioElement): void {
    this.interruptEl = el;
    el.addEventListener("ended", this.handleInterruptEnded);
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
    void this.ctx?.close();
    this.ctx = null;
  }

  /** Called after any user gesture so the AudioContext is allowed to run. */
  unlock(): void {
    this.ensureGraph();
    if (this.ctx?.state === "suspended") void this.ctx.resume();
  }

  setPresets(presets: PresetManifest[]): void {
    this.presets = new Map(presets.map((p) => [p.id, p]));
    // Re-apply if active presets were loaded after we got the state.
    if (this.lastActivePresetIds.length > 0) {
      this.rebuildEffectChain(this.lastActivePresetIds);
    }
  }

  // ----- main state apply ---------------------------------------------

  applyState(state: PlayerState, isMyOutput: boolean): void {
    this.isMyOutput = isMyOutput;
    this.crossfadeMs = state.crossfade_ms ?? 0;
    const t = state.crossfade_type;
    if (t === "linear" || t === "equal_power" || t === "cut") this.crossfadeType = t;

    // Volume changes apply unconditionally.
    if (state.volume !== this.lastVolume) {
      this.applyMasterVolume(state.volume);
      this.lastVolume = state.volume;
    }

    // Effect chain follows active_preset_ids.
    const activeIds = state.active_preset_ids ?? [];
    if (!sameStringList(activeIds, this.lastActivePresetIds)) {
      this.rebuildEffectChain(activeIds);
      this.lastActivePresetIds = [...activeIds];
    }

    // If this client isn't an output, ensure nothing's playing locally.
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

    // Stash interrupt fade values so the moment-of-fire knows them.
    if (state.interrupt) {
      this.pendingInterruptFadeIn = state.interrupt.fade_in_ms ?? 0;
      this.pendingInterruptFadeOut = state.interrupt.fade_out_ms ?? 0;
      this.lastInterruptFadeOut = this.pendingInterruptFadeOut;
    }

    // Interrupt transitions take precedence — they pause/resume ambient.
    if (newInterruptId !== this.lastInterruptId) {
      if (newInterruptId !== null) {
        this.startInterrupt(newInterruptId, this.pendingInterruptFadeIn);
        // Pause ambient while interrupt is live; we re-resume on end.
        this.pauseAmbient();
      } else {
        // Interrupt cleared — fade-out is normally handled by handleInterruptEnded
        // (called when the audio element ends). If the server cleared it
        // mid-track (cancel_interrupt), do an explicit fade here.
        if (this.lastInterruptFadeOut > 0) {
          this.fadeOutInterrupt(this.lastInterruptFadeOut);
        } else {
          this.stopInterrupt();
        }
      }
      this.lastInterruptId = newInterruptId;
    }

    // Ambient transitions while no interrupt is active.
    if (newInterruptId === null) {
      if (newAmbientId !== this.lastAmbientId) {
        if (newAmbientId === null) {
          this.stopAmbient();
        } else {
          // First time loading a track? No crossfade. Otherwise honour the setting.
          const useCrossfade =
            this.lastAmbientId !== null && newIsPlaying && this.crossfadeMs > 0;
          this.swapAmbient(newAmbientId, useCrossfade ? this.crossfadeMs : 0);
        }
        this.lastAmbientId = newAmbientId;
      }

      // Play / pause based on is_playing, respecting current source.
      if (newIsPlaying !== this.lastIsPlaying) {
        if (newIsPlaying) this.resumeAmbient();
        else this.pauseAmbient();
      }
    }

    this.lastIsPlaying = newIsPlaying;
    this.scheduleReports();
  }

  // ----- SFX ----------------------------------------------------------

  fireSfx(streamUrl: string, volume: number): void {
    if (!this.isMyOutput) return;
    this.ensureGraph();
    const ctx = this.ctx;
    if (ctx === null) return;
    const el = new Audio();
    el.crossOrigin = "use-credentials";
    el.src = streamUrl;
    // Each SFX gets its own gain node so concurrent SFX don't clip each
    // other's volumes.
    const gain = ctx.createGain();
    gain.gain.value = clampGain(volume);
    let src: MediaElementAudioSourceNode | null = null;
    try {
      src = ctx.createMediaElementSource(el);
    } catch {
      // Some browsers throw when reusing an element across contexts. Fall
      // back to letting the element route to the default device directly.
    }
    if (src && this.masterGain) {
      src.connect(gain).connect(this.masterGain);
    } else {
      el.volume = clampGain(volume);
    }
    const cleanup = (): void => {
      el.removeEventListener("ended", cleanup);
      el.removeEventListener("error", cleanup);
      try {
        src?.disconnect();
        gain.disconnect();
      } catch {
        /* node graph can throw on already-disconnected; ignore */
      }
    };
    el.addEventListener("ended", cleanup);
    el.addEventListener("error", cleanup);
    void el.play().catch(() => cleanup());
  }

  // ----- internals: graph wiring --------------------------------------

  private ensureGraph(): void {
    if (this.ctx !== null) return;
    if (!this.ambientA || !this.ambientB || !this.interruptEl) return;
    const Ctor =
      window.AudioContext ??
      (window as unknown as { webkitAudioContext: typeof AudioContext }).webkitAudioContext;
    const ctx = new Ctor();
    this.ctx = ctx;

    this.masterGain = ctx.createGain();
    this.masterGain.gain.value = clampGain(this.lastVolume);
    this.masterGain.connect(ctx.destination);

    // Effect chain endpoints — start as a pass-through (input === output)
    // and rebuild lazily when presets become active.
    this.effectInput = ctx.createGain();
    this.effectOutput = ctx.createGain();
    this.effectInput.connect(this.effectOutput);
    this.effectOutput.connect(this.masterGain);

    this.ambientA.src = ctx.createMediaElementSource(this.ambientA.el);
    this.ambientA.gain = ctx.createGain();
    this.ambientA.src.connect(this.ambientA.gain).connect(this.effectInput);
    this.ambientA.gain.gain.value = 1;

    this.ambientB.src = ctx.createMediaElementSource(this.ambientB.el);
    this.ambientB.gain = ctx.createGain();
    this.ambientB.src.connect(this.ambientB.gain).connect(this.effectInput);
    this.ambientB.gain.gain.value = 0;

    this.interruptSrc = ctx.createMediaElementSource(this.interruptEl);
    this.interruptGain = ctx.createGain();
    this.interruptGain.gain.value = 0;
    this.interruptSrc.connect(this.interruptGain).connect(this.masterGain);
  }

  private applyMasterVolume(volume: number): void {
    this.lastVolume = volume;
    if (this.masterGain && this.ctx) {
      this.masterGain.gain.cancelScheduledValues(this.ctx.currentTime);
      this.masterGain.gain.setValueAtTime(clampGain(volume), this.ctx.currentTime);
    }
  }

  // ----- internals: ambient -------------------------------------------

  private currentChannel(): AmbientChannel | null {
    return this.currentSlot === "A" ? this.ambientA : this.ambientB;
  }
  private otherChannel(): AmbientChannel | null {
    return this.currentSlot === "A" ? this.ambientB : this.ambientA;
  }

  /** Load a track URL into the named channel, pausing first to avoid the
   *  brief "old track tail" you get when changing src mid-play. */
  private loadInto(ch: AmbientChannel, streamUrl: string): void {
    ch.el.pause();
    ch.el.crossOrigin = "use-credentials";
    if (!ch.el.src.endsWith(streamUrl)) {
      ch.el.src = streamUrl;
      ch.el.load();
    }
  }

  private swapAmbient(trackId: number, crossfadeMs: number): void {
    this.ensureGraph();
    const url = `/api/library/tracks/${trackId}/stream`;

    const current = this.currentChannel();
    const other = this.otherChannel();
    if (!current || !other) return;

    if (crossfadeMs <= 0 || !this.ctx) {
      // Snap-cut: load on the current channel, keep gain at 1.
      this.loadInto(current, url);
      this.setGainNow(current, 1);
      this.setGainNow(other, 0);
      if (this.lastIsPlaying) void current.el.play().catch(() => undefined);
      return;
    }

    // Crossfade: load on the OTHER channel, ramp gains over crossfadeMs.
    this.loadInto(other, url);
    void other.el.play().catch(() => undefined);
    this.rampGain(current, current.gain?.gain.value ?? 1, 0, crossfadeMs);
    this.rampGain(other, other.gain?.gain.value ?? 0, 1, crossfadeMs);
    // Stop the outgoing element after the fade.
    window.setTimeout(() => {
      current.el.pause();
    }, crossfadeMs);
    this.currentSlot = this.currentSlot === "A" ? "B" : "A";
  }

  private resumeAmbient(): void {
    if (this.lastInterruptId !== null) return;
    void this.ctx?.resume();
    const ch = this.currentChannel();
    if (ch) void ch.el.play().catch(() => undefined);
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
    if (!ch.gain || !this.ctx) {
      ch.el.volume = clampGain(value);
      return;
    }
    ch.gain.gain.cancelScheduledValues(this.ctx.currentTime);
    ch.gain.gain.setValueAtTime(clampGain(value), this.ctx.currentTime);
  }

  private rampGain(ch: AmbientChannel, from: number, to: number, ms: number): void {
    if (!ch.gain || !this.ctx) {
      ch.el.volume = clampGain(to);
      return;
    }
    const now = this.ctx.currentTime;
    const dur = Math.max(0.01, ms / 1000);
    ch.gain.gain.cancelScheduledValues(now);
    ch.gain.gain.setValueAtTime(clampGain(from), now);
    if (this.crossfadeType === "equal_power") {
      // Approximate equal-power: exponential ramp avoids the audible dip.
      ch.gain.gain.exponentialRampToValueAtTime(Math.max(0.0001, clampGain(to)), now + dur);
      // Snap to 0 at the very end if target is 0 (exponential can't reach it).
      if (to === 0) ch.gain.gain.setValueAtTime(0, now + dur + 0.01);
    } else {
      ch.gain.gain.linearRampToValueAtTime(clampGain(to), now + dur);
    }
  }

  // ----- internals: interrupt -----------------------------------------

  private startInterrupt(trackId: number, fadeInMs: number): void {
    this.ensureGraph();
    if (!this.interruptEl || !this.interruptGain || !this.ctx) return;
    const url = `/api/library/tracks/${trackId}/stream`;
    this.interruptEl.crossOrigin = "use-credentials";
    if (!this.interruptEl.src.endsWith(url)) {
      this.interruptEl.src = url;
      this.interruptEl.load();
    }
    const now = this.ctx.currentTime;
    this.interruptGain.gain.cancelScheduledValues(now);
    if (fadeInMs > 0) {
      this.interruptGain.gain.setValueAtTime(0, now);
      this.interruptGain.gain.linearRampToValueAtTime(1, now + fadeInMs / 1000);
    } else {
      this.interruptGain.gain.setValueAtTime(1, now);
    }
    void this.ctx.resume();
    void this.interruptEl.play().catch(() => undefined);
  }

  private fadeOutInterrupt(ms: number): void {
    if (!this.interruptEl || !this.interruptGain || !this.ctx) return;
    const now = this.ctx.currentTime;
    const dur = Math.max(0.01, ms / 1000);
    this.interruptGain.gain.cancelScheduledValues(now);
    this.interruptGain.gain.setValueAtTime(this.interruptGain.gain.value, now);
    this.interruptGain.gain.linearRampToValueAtTime(0, now + dur);
    window.setTimeout(() => this.stopInterrupt(), ms);
  }

  private stopInterrupt(): void {
    if (this.interruptEl) {
      this.interruptEl.pause();
      this.interruptEl.removeAttribute("src");
      this.interruptEl.load();
    }
    if (this.interruptGain && this.ctx) {
      this.interruptGain.gain.setValueAtTime(0, this.ctx.currentTime);
    }
    // Resume ambient at preserved position if the player thinks playback
    // is on. Server sets is_playing depending on return_to_ambient.
    if (this.lastIsPlaying) {
      const ch = this.currentChannel();
      if (ch) void ch.el.play().catch(() => undefined);
    }
  }

  private silenceAll(): void {
    this.pauseAmbient();
    if (this.interruptEl) this.interruptEl.pause();
    if (this.interruptGain && this.ctx) {
      this.interruptGain.gain.setValueAtTime(0, this.ctx.currentTime);
    }
  }

  // ----- internals: effect chain --------------------------------------

  private rebuildEffectChain(activeIds: string[]): void {
    if (!this.ctx || !this.effectInput || !this.effectOutput || !this.masterGain) return;

    // Disconnect the old chain.
    try {
      this.effectInput.disconnect();
    } catch {
      /* may not be connected yet */
    }
    try {
      this.effectOutput.disconnect();
    } catch {
      /* may not be connected yet */
    }

    // Build a fresh chain of nodes from the active presets, in order.
    const nodes: AudioNode[] = [];
    for (const id of activeIds) {
      const preset = this.presets.get(id);
      if (!preset) continue;
      for (const eff of preset.effects) {
        const node = this.makeEffectNode(eff);
        if (node) nodes.push(...node);
      }
    }

    // Wire input → ...nodes → output → master.
    let last: AudioNode = this.effectInput;
    for (const n of nodes) {
      last.connect(n);
      last = n;
    }
    last.connect(this.effectOutput);
    this.effectOutput.connect(this.masterGain);
  }

  /** Build the AudioNode(s) for a single effect spec. Returns an array
   *  because some effects need multiple nodes wired in series (e.g. delay
   *  with a feedback gain). */
  private makeEffectNode(eff: EffectSpec): AudioNode[] | null {
    const ctx = this.ctx;
    if (!ctx) return null;
    const num = (k: string, fallback: number): number => {
      const v = eff[k];
      return typeof v === "number" ? v : fallback;
    };
    switch (eff.type) {
      case "lowpass":
      case "highpass":
      case "bandpass": {
        const f = ctx.createBiquadFilter();
        f.type = eff.type;
        f.frequency.value = num("frequency", eff.type === "lowpass" ? 800 : 200);
        f.Q.value = num("q", 0.7);
        return [f];
      }
      case "delay": {
        const d = ctx.createDelay(5);
        d.delayTime.value = num("time", 0.25);
        const fb = ctx.createGain();
        fb.gain.value = clampGain(num("feedback", 0.3));
        const wet = ctx.createGain();
        wet.gain.value = clampGain(num("wet", 0.4));
        d.connect(fb).connect(d);
        d.connect(wet);
        return [d, wet];
      }
      case "distortion": {
        const ws = ctx.createWaveShaper();
        ws.curve = makeDistortionCurve(num("amount", 50));
        ws.oversample = "4x";
        return [ws];
      }
      case "tremolo": {
        // Tremolo: an LFO modulates a gain node's gain.
        const lfo = ctx.createOscillator();
        lfo.frequency.value = num("rate", 5);
        const lfoGain = ctx.createGain();
        lfoGain.gain.value = clampGain(num("depth", 0.5));
        const tremGain = ctx.createGain();
        tremGain.gain.value = 1 - clampGain(num("depth", 0.5));
        lfo.connect(lfoGain).connect(tremGain.gain);
        lfo.start();
        return [tremGain];
      }
      case "reverb": {
        const conv = ctx.createConvolver();
        conv.buffer = makeImpulseResponse(
          ctx,
          num("decay", 2.0),
          num("reverse", 0) === 1,
        );
        const wet = ctx.createGain();
        wet.gain.value = clampGain(num("wet", 0.4));
        conv.connect(wet);
        return [conv, wet];
      }
      case "pitch_shift": {
        // Web Audio has no native pitch shifter; deferred (see docs/FUTURE.md).
        // Skip silently so the rest of the chain still works.
        return null;
      }
      default:
        return null;
    }
  }

  // ----- internals: position reports ----------------------------------

  private scheduleReports(): void {
    if (this.reportTimer !== null) return;
    this.reportTimer = window.setInterval(() => {
      if (!this.isMyOutput || !this.lastIsPlaying || !this.onPositionReport) return;
      const ms = this.currentPositionMs();
      if (isFiniteNonNeg(ms)) this.onPositionReport(Math.floor(ms));
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

  // ----- DOM event handlers (bound) -----------------------------------

  private handleAmbientEnded = (e: Event): void => {
    // Only react if it's the current channel — the other one ends due to
    // crossfade fade-out and we ignore that.
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

// ----- helpers --------------------------------------------------------

function sameStringList(a: string[], b: string[]): boolean {
  if (a.length !== b.length) return false;
  for (let i = 0; i < a.length; i++) if (a[i] !== b[i]) return false;
  return true;
}

function makeDistortionCurve(amount: number): Float32Array<ArrayBuffer> {
  // Standard "soft clipping" curve. Higher `amount` → more saturation.
  // Construct over a concrete ArrayBuffer (not ArrayBufferLike) so TS
  // accepts it for WaveShaperNode.curve under strict lib defs.
  const k = Math.max(0, amount);
  const samples = 4096;
  const buffer = new ArrayBuffer(samples * 4);
  const curve = new Float32Array(buffer);
  const deg = Math.PI / 180;
  for (let i = 0; i < samples; i++) {
    const x = (i * 2) / samples - 1;
    curve[i] = ((3 + k) * x * 20 * deg) / (Math.PI + k * Math.abs(x));
  }
  return curve;
}

function makeImpulseResponse(
  ctx: AudioContext,
  decaySeconds: number,
  reverse: boolean,
): AudioBuffer {
  // Synthesised IR: stereo white noise multiplied by an exponential decay.
  // Good enough for "room"-flavoured reverb without shipping an audio file.
  const rate = ctx.sampleRate;
  const length = Math.max(1, Math.floor(rate * Math.max(0.05, decaySeconds)));
  const ir = ctx.createBuffer(2, length, rate);
  for (let ch = 0; ch < 2; ch++) {
    const data = ir.getChannelData(ch);
    for (let i = 0; i < length; i++) {
      const t = reverse ? length - i : i;
      data[i] = (Math.random() * 2 - 1) * Math.pow(1 - t / length, 2);
    }
  }
  return ir;
}

// Module-level singleton — one engine per browser tab, just like the WS.
export const playbackEngine = new PlaybackEngine();
