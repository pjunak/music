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
 * intent is "preset effects colour the background music; alerts and stings
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
  /** Last URL the channel was loaded with. Tracked explicitly because
   *  `el.src` can be normalised by the browser (resolved against the
   *  document base, query params re-ordered, etc.), so an `endsWith`
   *  comparison is fragile. */
  loadedUrl: string | null;
}

interface InterruptChannel {
  el: HTMLAudioElement;
  gainNode: GainNode;
  source: MediaElementAudioSourceNode;
  rampToken: number;
  loadedUrl: string | null;
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

/** Drift tolerance for remote-seek detection and snapping. */
const REMOTE_SEEK_THRESHOLD_MS = 1500;

/** Minimum gap between two queue-advances. Stops a single track-end from
 *  being acted on twice when the `ended` event and the stall backstop race,
 *  and absorbs the round-trip before the next track's state arrives (during
 *  which the stalled element is still the current channel). Comfortably below
 *  the length of any real track so `loop: track` still re-advances. */
const ADVANCE_DEBOUNCE_MS = 2000;

/** How close to the end (seconds) the element must be before the stall
 *  backstop will consider advancing. Scoped tight so a mid-track buffer
 *  underrun is never mistaken for end-of-track. */
const END_STALL_WINDOW_S = 1.0;

/** Per-poll progress (seconds) below which playback counts as "not
 *  advancing". Generous enough to ignore sub-frame jitter. */
const END_STALL_EPSILON_S = 0.05;

function clamp01(v: number): number {
  return Math.max(0, Math.min(1, v));
}

/**
 * Decide whether an incoming server position represents a genuine remote
 * seek that the locally-playing element should snap to.
 *
 * The output device is the source of truth for its own playback position.
 * Routine `state_changed` broadcasts (volume, queue edits, mode switches, …)
 * are NOT seeks — the server doesn't dead-reckon `position_ms`, so each one
 * carries back the position this device itself last *reported*. Comparing
 * that echo against the live element time would false-positive on every
 * unrelated change and yank playback backward — on a sluggish TV, all the
 * way to the start (its reports lag, so the echoed position is far behind).
 *
 * So we gate on divergence from our OWN telemetry instead: a real seek (the
 * DM dragging the scrub bar, a `loop: track` restart) moves the server
 * position away from what we reported; an echo does not. `lastReportedMs`
 * only advances on reports that actually reached the wire, so a dropped send
 * during a reconnect can't make a later echo look like a seek.
 */
export function shouldApplyRemoteSeek(args: {
  /** Server-broadcast position for the still-current track. */
  targetMs: number;
  /** Position this device last successfully reported to the server. */
  lastReportedMs: number;
  /** The element's live `currentTime`, in ms. */
  elapsedMs: number;
  thresholdMs: number;
}): boolean {
  const { targetMs, lastReportedMs, elapsedMs, thresholdMs } = args;
  if (!Number.isFinite(elapsedMs)) return false;
  // Gate: only a position that diverged from our telemetry is a real seek.
  if (Math.abs(targetMs - lastReportedMs) <= thresholdMs) return false;
  // Apply: skip the snap if the element already sits at the target, so a
  // redundant broadcast doesn't cause a needless re-seek.
  if (Math.abs(targetMs - elapsedMs) <= thresholdMs) return false;
  return true;
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
  /** Duck-control gain between the effect chain tail and `ambientMaster`.
   *  Stays at 1.0 in normal playback; ramps down to a partial level when
   *  an interrupt fires with `duck_to` set, so ambient music continues
   *  quietly under the interrupt instead of pausing. Master volume stays
   *  on `ambientMaster` so duck and master volume don't interfere. */
  private ambientDuck: GainNode | null = null;
  /** Master ambient gain node — drives master volume and is the graph's
   *  terminal node before `destination`. */
  private ambientMaster: GainNode | null = null;
  /** Master interrupt gain node — bypasses the effect chain. */
  private interruptMaster: GainNode | null = null;

  /** This browser's stable client_id, so this device's per-device volume trim
   *  (`PlayerState.device_volumes[clientId]`) can be folded into master gain.
   *  Set once at mount via `setClientId`. */
  private clientId = "";

  /** Currently inserted effect graphs. Disposed when presets change. */
  private installedEffects: BuiltEffect[] = [];

  // --- last-applied state, used to short-circuit no-op state changes -----
  private lastAmbientId: number | null = null;
  private lastInterruptId: number | null = null;
  private lastIsPlaying = false;
  private lastVolume = 1.0;
  private lastInterruptFadeOut = 0;
  private lastPresetSignature = "";
  /** Position this device last reported to the server *and the server
   *  accepted* (i.e. the WS send actually went out). The baseline for
   *  remote-seek detection — see `shouldApplyRemoteSeek`. Reset to 0 on
   *  every fresh track load because the server zeroes `position_ms` on a
   *  track change. */
  private lastReportedMs = 0;
  /** `performance.now()` of the last queue-advance we triggered. Debounces
   *  the `ended` event against the stall backstop. */
  private lastAdvanceAt = -Infinity;
  /** `currentTime` (seconds) observed on the previous report tick while
   *  within the end-of-track window, or -1 when not armed. Lets the stall
   *  backstop require two consecutive no-progress polls before acting. */
  private endStallTime = -1;
  /** When non-null, an interrupt is currently ducking ambient (rather
   *  than pausing it). Records the target level so the un-duck path
   *  knows whether it has work to do on interrupt-end. */
  private currentDuckTo: number | null = null;
  /** Cancellation token for the most recently scheduled duck/un-duck rAF
   *  ramp. Bumped before each new ramp so an in-flight one drops on the
   *  floor rather than fighting the new target. */
  private duckRampToken = 0;

  private isMyOutput = false;

  private crossfadeMs = 0;
  private crossfadeType: "linear" | "equal_power" | "cut" = "linear";

  private reportTimer: number | null = null;

  private onSkipNext: (() => void) | null = null;
  private onInterruptSkipNext: (() => void) | null = null;
  /** Reports the position upstream and returns whether it actually went out
   *  (false if the socket is mid-reconnect). The return value gates
   *  `lastReportedMs` so a dropped report doesn't desync seek detection. */
  private onPositionReport: ((ms: number) => boolean) | null = null;

  // ----- wiring -------------------------------------------------------

  setAmbientElements(a: HTMLAudioElement, b: HTMLAudioElement): void {
    const ctx = this.ensureAudioContext();
    if (ctx === null) {
      // Web Audio unavailable — bail; we can't drive ambient without a graph.
      console.warn("[playbackEngine] AudioContext unavailable; ambient muted");
      return;
    }
    // Idempotent re-entry. React StrictMode (dev) double-invokes the mounting
    // effect with the SAME <audio> elements; `createMediaElementSource` throws
    // if called twice on one element ("already connected"). Reuse the existing
    // source nodes and just re-attach the "ended" listeners destroy() detached
    // (addEventListener dedupes the stable bound ref). In production this method
    // runs once (ambientA is null here), so the guard is inert and the live
    // audio graph is built exactly as before.
    if (this.ambientA?.el === a && this.ambientB?.el === b) {
      a.addEventListener("ended", this.handleAmbientEnded);
      b.addEventListener("ended", this.handleAmbientEnded);
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
    this.ambientA = { el: a, gainNode: gainA, source: sourceA, rampToken: 0, loadedUrl: null };
    this.ambientB = { el: b, gainNode: gainB, source: sourceB, rampToken: 0, loadedUrl: null };
    a.addEventListener("ended", this.handleAmbientEnded);
    b.addEventListener("ended", this.handleAmbientEnded);
    attachErrorLogger("ambientA", a);
    attachErrorLogger("ambientB", b);
  }

  setInterruptElement(el: HTMLAudioElement): void {
    const ctx = this.ensureAudioContext();
    if (ctx === null) return;
    // Idempotent re-entry — see setAmbientElements. Inert in production.
    if (this.interrupt?.el === el) {
      el.addEventListener("ended", this.handleInterruptEnded);
      return;
    }
    el.volume = 1;
    const source = ctx.createMediaElementSource(el);
    const gainNode = ctx.createGain();
    gainNode.gain.value = 0;
    source.connect(gainNode);
    if (this.interruptMaster !== null) {
      gainNode.connect(this.interruptMaster);
    }
    this.interrupt = { el, gainNode, source, rampToken: 0, loadedUrl: null };
    el.addEventListener("ended", this.handleInterruptEnded);
    attachErrorLogger("interrupt", el);
  }

  setHandlers(handlers: {
    onSkipNext: () => void;
    onInterruptSkipNext: () => void;
    onPositionReport: (ms: number) => boolean;
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
    this.ambientDuck = ctx.createGain();
    this.ambientMaster.gain.value = clamp01(this.lastVolume);
    this.interruptMaster.gain.value = clamp01(this.lastVolume);
    this.ambientDuck.gain.value = 1;
    this.ambientMaster.connect(ctx.destination);
    this.interruptMaster.connect(ctx.destination);
    this.ambientDuck.connect(this.ambientMaster);
    // No effects yet — passthrough head → duck → master.
    this.effectChainHead.connect(this.ambientDuck);
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
    if (
      ctx === null ||
      this.effectChainHead === null ||
      this.ambientDuck === null ||
      this.ambientMaster === null
    ) {
      return; // Web Audio not available or not wired yet — no-op.
    }
    // Cheap fingerprint so we can skip rebuilds when the active presets
    // didn't actually change. setPresets gets called on every state push.
    const signature = JSON.stringify(
      presets.map((p) => ({ id: p.id, effects: p.effects })),
    );
    if (signature === this.lastPresetSignature) return;
    this.lastPresetSignature = signature;

    // Tear down the current chain. We rebuild the head→…→duck path here;
    // duck→master and master→destination are stable, no need to touch.
    this.disposeEffects();
    this.effectChainHead.disconnect();

    // Flatten preset chains in declaration order — preset[0].effects then
    // preset[1].effects, etc. Skip unsupported effects; warn once per type.
    const flat: EffectSpec[] = [];
    for (const preset of presets) {
      for (const eff of preset.effects) {
        flat.push(eff);
      }
    }

    if (flat.length === 0) {
      // Empty chain — passthrough head → duck.
      this.effectChainHead.connect(this.ambientDuck);
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
    upstream.connect(this.ambientDuck);
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
    const wasMyOutput = this.isMyOutput;
    this.isMyOutput = isMyOutput;
    this.crossfadeMs = state.crossfade_ms ?? 0;
    const t = state.crossfade_type;
    if (t === "linear" || t === "equal_power" || t === "cut") this.crossfadeType = t;

    // Effective volume = master × this device's per-device trim (absent = 1.0),
    // so the operator can tame a too-loud TV from the Console without touching
    // master. Folded into lastVolume so SFX (which read it) scale too.
    const trim = state.device_volumes?.[this.clientId] ?? 1;
    const effective = clamp01(state.volume) * clamp01(trim);
    if (effective !== this.lastVolume) {
      this.lastVolume = effective;
      this.applyMasterVolume();
    }

    if (!isMyOutput) {
      // Only do the heavy teardown on the actual ON → OFF transition.
      // (`silenceAll` is idempotent but `releaseOutput` calls `el.load()`
      // which is best avoided in the steady state.)
      if (wasMyOutput) {
        this.releaseOutput();
      }
      this.lastAmbientId = null;
      this.lastInterruptId = null;
      this.lastIsPlaying = false;
      this.lastReportedMs = 0;
      this.endStallTime = -1;
      return;
    }

    const newAmbientId = state.ambient.current_track_id ?? null;
    const newInterruptId = state.interrupt?.current_track_id ?? null;
    const newIsPlaying = state.is_playing;
    const prevAmbientId = this.lastAmbientId;
    const prevInterruptId = this.lastInterruptId;

    if (state.interrupt) {
      this.lastInterruptFadeOut = state.interrupt.fade_out_ms ?? 0;
    }

    if (newInterruptId !== this.lastInterruptId) {
      if (newInterruptId !== null) {
        const fadeIn = state.interrupt?.fade_in_ms ?? 0;
        const duckTo = state.interrupt?.duck_to ?? null;
        this.startInterrupt(newInterruptId, fadeIn);
        if (duckTo !== null) {
          // Cinematic mode: keep ambient playing, just lower its volume.
          this.duckAmbient(duckTo, fadeIn);
          this.currentDuckTo = duckTo;
        } else {
          // Legacy mode: ambient pauses for the duration of the interrupt.
          this.pauseAmbient();
          this.currentDuckTo = null;
        }
      } else if (this.lastInterruptFadeOut > 0) {
        this.fadeOutInterrupt(this.lastInterruptFadeOut);
        if (this.currentDuckTo !== null) {
          this.unduckAmbient(this.lastInterruptFadeOut);
          this.currentDuckTo = null;
        }
      } else {
        this.stopInterrupt();
        if (this.currentDuckTo !== null) {
          this.unduckAmbient(0);
          this.currentDuckTo = null;
        }
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

    // Seek detection on a same-track broadcast. A genuine seek (operator drags
    // the scrub bar, /loop seek action) moves the server position away from
    // our telemetry; an unrelated change (volume, queue edit) just echoes the
    // position we last reported. `maybeSeek` distinguishes the two — see
    // `shouldApplyRemoteSeek` — so this no longer false-fires and restarts the
    // track on every state change.
    if (newInterruptId !== null && newInterruptId === prevInterruptId && state.interrupt) {
      this.maybeSeek(this.interrupt?.el ?? null, state.interrupt.position_ms);
    } else if (
      newInterruptId === null &&
      newAmbientId !== null &&
      newAmbientId === prevAmbientId
    ) {
      this.maybeSeek(this.currentChannel()?.el ?? null, state.ambient.position_ms);
    }

    this.lastIsPlaying = newIsPlaying;
    this.scheduleReports();
  }

  private maybeSeek(el: HTMLAudioElement | null, targetMs: number): void {
    if (el === null) return;
    const elapsedMs = Number.isFinite(el.currentTime) ? el.currentTime * 1000 : NaN;
    if (
      !shouldApplyRemoteSeek({
        targetMs,
        lastReportedMs: this.lastReportedMs,
        elapsedMs,
        thresholdMs: REMOTE_SEEK_THRESHOLD_MS,
      })
    ) {
      return;
    }
    el.currentTime = Math.max(0, targetMs / 1000);
    // The snap target is now where the server believes us to be, so treat it
    // as our telemetry baseline — otherwise the seek's own echo in the next
    // broadcast would re-trigger.
    this.lastReportedMs = Math.floor(targetMs);
  }

  /** Ramp the ambient duck gain to `toLevel` over `ms`. `toLevel` of 0.3
   * means ambient plays at 30% during an interrupt — combined with master
   * volume, you get a cinematic "background music" feel rather than a
   * hard pause. */
  private duckAmbient(toLevel: number, ms: number): void {
    this.rampDuck(clamp01(toLevel), ms);
  }

  /** Counterpart to `duckAmbient` — ramp back to 1.0 (no attenuation)
   * over `ms`. Called on interrupt-end if we were ducking. */
  private unduckAmbient(ms: number): void {
    this.rampDuck(1, ms);
  }

  private rampDuck(toLevel: number, ms: number): void {
    if (this.ambientDuck === null) return;
    const node = this.ambientDuck;
    this.duckRampToken += 1;
    const myToken = this.duckRampToken;
    const from = node.gain.value;
    if (ms <= 0) {
      node.gain.value = toLevel;
      return;
    }
    const start = performance.now();
    const tick = (now: number) => {
      if (myToken !== this.duckRampToken) return;
      const t = Math.min(1, (now - start) / ms);
      node.gain.value = from + (toLevel - from) * t;
      if (t < 1) window.requestAnimationFrame(tick);
    };
    window.requestAnimationFrame(tick);
  }

  /** Surrender the active-output role: pause every channel and tear the
   * src off each <audio> so it stops audibly *and* so a subsequent re-claim
   * triggers a fresh load. Without the unload, a simple `pause()` would
   * leave the element with its old src — when we toggle back on, the
   * `loadInto` short-circuit (`src.endsWith(url)`) skips reload and
   * `safePlay` resumes from the same paused position, which manifests as
   * "toggle does nothing until refresh". */
  private releaseOutput(): void {
    this.pauseAmbient();
    if (this.ambientA?.el) {
      this.ambientA.el.removeAttribute("src");
      this.ambientA.el.load();
      this.ambientA.loadedUrl = null;
    }
    if (this.ambientB?.el) {
      this.ambientB.el.removeAttribute("src");
      this.ambientB.el.load();
      this.ambientB.loadedUrl = null;
    }
    if (this.interrupt) {
      this.interrupt.el.pause();
      this.interrupt.el.removeAttribute("src");
      this.interrupt.el.load();
      this.interrupt.loadedUrl = null;
      this.interrupt.gainNode.gain.value = 0;
    }
    // Cancel any in-flight duck ramp and reset the duck gain so a
    // subsequent re-claim doesn't start with ambient at 30%.
    this.duckRampToken += 1;
    if (this.ambientDuck !== null) this.ambientDuck.gain.value = 1;
    this.currentDuckTo = null;
    this.lastReportedMs = 0;
    this.endStallTime = -1;
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

  /** Tell the engine this browser's client_id so it can apply this device's
   *  per-device volume trim. Idempotent; safe to call on every mount. */
  setClientId(id: string): void {
    this.clientId = id;
  }

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
    if (ch.loadedUrl !== streamUrl) {
      ch.el.src = streamUrl;
      ch.el.load();
      ch.loadedUrl = streamUrl;
    }
  }

  private swapAmbient(trackId: number, crossfadeMs: number): void {
    const url = `/api/library/tracks/${trackId}/stream`;
    const current = this.currentChannel();
    const other = this.otherChannel();
    if (!current || !other) return;

    // A track change zeroes the server's position_ms, and the element starts
    // at 0 — reset the seek baseline and disarm the stall backstop to match.
    this.lastReportedMs = 0;
    this.endStallTime = -1;

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
    if (this.ambientA?.el) {
      this.ambientA.el.removeAttribute("src");
      this.ambientA.el.load();
      this.ambientA.loadedUrl = null;
    }
    if (this.ambientB?.el) {
      this.ambientB.el.removeAttribute("src");
      this.ambientB.el.load();
      this.ambientB.loadedUrl = null;
    }
    this.lastReportedMs = 0;
    this.endStallTime = -1;
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
    // Reports now flow to the interrupt lane, which starts at 0 — reset the
    // shared seek baseline so the first interrupt broadcast isn't read as a
    // seek.
    this.lastReportedMs = 0;
    const url = `/api/library/tracks/${trackId}/stream`;
    if (this.interrupt.loadedUrl !== url) {
      this.interrupt.el.src = url;
      this.interrupt.el.load();
      this.interrupt.loadedUrl = url;
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
      this.interrupt.loadedUrl = null;
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
      if (!this.isMyOutput || !this.lastIsPlaying) return;
      // Backstop for browsers that don't reliably fire `ended` (older smart
      // TVs / WebViews): if playback has stalled at the tail, advance the
      // queue ourselves rather than waiting for an event that isn't coming.
      this.maybeAdvanceAtEnd();
      if (!this.onPositionReport) return;
      const ms = this.currentPositionMs();
      if (!Number.isFinite(ms) || ms < 0) return;
      const floored = Math.floor(ms);
      // Only treat the position as "known to the server" once the report is
      // actually on the wire. A mid-reconnect send is a silent no-op;
      // advancing the baseline anyway would later make an unrelated broadcast
      // (which echoes the server's stale position) look like a remote seek.
      if (this.onPositionReport(floored)) {
        this.lastReportedMs = floored;
      }
    }, POSITION_REPORT_INTERVAL_MS);
  }

  /** Stall backstop. Fires a queue-advance when the current ambient element
   *  has reached the tail of the track but isn't progressing — the case where
   *  a flaky browser never delivers `ended`. Requires two consecutive
   *  no-progress polls inside the end window, so healthy playback (which is
   *  still advancing right up to the real `ended`) never trips it and never
   *  loses its final second. */
  private maybeAdvanceAtEnd(): void {
    if (this.lastInterruptId !== null) {
      this.endStallTime = -1;
      return; // interrupts run their own end handling
    }
    const ch = this.currentChannel();
    const el = ch?.el;
    if (!el || el.paused) {
      this.endStallTime = -1;
      return;
    }
    const dur = el.duration;
    const now = el.currentTime;
    if (!Number.isFinite(dur) || dur <= 0 || !Number.isFinite(now)) {
      this.endStallTime = -1;
      return; // unknown duration → can't tell the tail from the middle
    }
    if (dur - now > END_STALL_WINDOW_S) {
      this.endStallTime = -1;
      return; // not near the end
    }
    if (this.endStallTime >= 0 && now <= this.endStallTime + END_STALL_EPSILON_S) {
      // Near the end and not advancing since the previous poll → stalled.
      this.endStallTime = -1;
      this.fireAmbientAdvance();
      return;
    }
    this.endStallTime = now; // arm: re-check on the next poll
  }

  /** Single funnel for advancing the ambient queue, debounced so the `ended`
   *  event and the stall backstop can't double-skip a track (and so the
   *  in-flight round-trip to load the next track doesn't get re-triggered). */
  private fireAmbientAdvance(): void {
    const now = performance.now();
    if (now - this.lastAdvanceAt < ADVANCE_DEBOUNCE_MS) return;
    this.lastAdvanceAt = now;
    this.onSkipNext?.();
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
      this.fireAmbientAdvance();
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
