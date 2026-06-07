/** Graphic-EQ math, shared by the audio engine (which builds real
 *  `BiquadFilter` peaking nodes) and the editor's response-curve preview
 *  (which draws the summed magnitude response). Keeping the band set + the
 *  transfer-function math in one place means the curve you drag in the editor
 *  matches what the engine actually applies.
 *
 *  A graphic EQ is a series of one-octave peaking filters at fixed ISO-ish
 *  centre frequencies; each band's gain (dB) is one vertical fader. The
 *  response is computed analytically from the RBJ peaking-EQ biquad (Audio EQ
 *  Cookbook) so it needs no AudioContext — it runs anywhere, including tests. */

export interface EqBand {
  /** Centre frequency in Hz (one of EQ_FREQUENCIES). */
  frequency: number;
  /** Boost/cut in dB. */
  gain: number;
}

// Ten octave-spaced bands — the classic graphic-EQ layout.
export const EQ_FREQUENCIES = [
  32, 64, 125, 250, 500, 1000, 2000, 4000, 8000, 16000,
] as const;

// Compact fader captions (Hz below 1k, kHz above).
export const EQ_BAND_LABELS = [
  "32",
  "64",
  "125",
  "250",
  "500",
  "1k",
  "2k",
  "4k",
  "8k",
  "16k",
];

export const EQ_GAIN_MIN = -12;
export const EQ_GAIN_MAX = 12;
export const EQ_GAIN_STEP = 0.5;

// Q for a ~one-octave peaking band: Q = sqrt(2^N)/(2^N - 1) with N = 1.
export const EQ_BAND_Q = 1.414;

// Sample rate used for the editor's analytic preview. The engine uses the live
// AudioContext rate; the only divergence is a hair near Nyquist — invisible on
// the curve. 48 kHz is the common browser default.
const PREVIEW_FS = 48000;

export function clampGain(db: number): number {
  if (!Number.isFinite(db)) return 0;
  return Math.max(EQ_GAIN_MIN, Math.min(EQ_GAIN_MAX, db));
}

export function defaultEqBands(): EqBand[] {
  return EQ_FREQUENCIES.map((frequency) => ({ frequency, gain: 0 }));
}

/** Coerce a loaded preset's `bands` (arbitrary JSON) into exactly the canonical
 *  band set, in order. Missing / malformed entries default to flat (0 dB) so a
 *  hand-edited or partial YAML never blanks the editor. */
export function normalizeEqBands(raw: unknown): EqBand[] {
  const arr = Array.isArray(raw) ? raw : [];
  return EQ_FREQUENCIES.map((frequency, i) => {
    const entry = arr[i] as { gain?: unknown } | undefined;
    const gain = entry && typeof entry.gain === "number" ? entry.gain : 0;
    return { frequency, gain: clampGain(gain) };
  });
}

/** True when every band is flat — lets callers skip building/serialising a
 *  no-op EQ. */
export function isFlat(bands: EqBand[]): boolean {
  return bands.every((b) => b.gain === 0);
}

/** Magnitude response (dB) of one RBJ peaking-EQ biquad at `freq`. */
function peakingResponseDb(
  freq: number,
  f0: number,
  q: number,
  gainDb: number,
  fs: number,
): number {
  if (gainDb === 0) return 0; // flat band contributes nothing
  const A = Math.pow(10, gainDb / 40);
  const w0 = (2 * Math.PI * f0) / fs;
  const cw0 = Math.cos(w0);
  const alpha = Math.sin(w0) / (2 * q);

  // Cookbook peaking-EQ coefficients (unnormalised; the a0 term stays in the
  // denominator polynomial below so no separate normalisation is needed).
  const b0 = 1 + alpha * A;
  const b1 = -2 * cw0;
  const b2 = 1 - alpha * A;
  const a0 = 1 + alpha / A;
  const a1 = -2 * cw0;
  const a2 = 1 - alpha / A;

  // Evaluate H(e^{jw}) = (b0 + b1 z^-1 + b2 z^-2) / (a0 + a1 z^-1 + a2 z^-2).
  const w = (2 * Math.PI * freq) / fs;
  const cosw = Math.cos(w);
  const sinw = Math.sin(w);
  const cos2w = Math.cos(2 * w);
  const sin2w = Math.sin(2 * w);

  const numRe = b0 + b1 * cosw + b2 * cos2w;
  const numIm = -(b1 * sinw + b2 * sin2w);
  const denRe = a0 + a1 * cosw + a2 * cos2w;
  const denIm = -(a1 * sinw + a2 * sin2w);

  const numMag = Math.hypot(numRe, numIm);
  const denMag = Math.hypot(denRe, denIm);
  if (denMag === 0) return 0;
  return 20 * Math.log10(numMag / denMag);
}

/** Summed magnitude response (dB) of the whole band set at each frequency in
 *  `freqsHz`. Peaking filters in series multiply (linear) → add (dB). */
export function computeEqResponseDb(
  bands: EqBand[],
  freqsHz: ArrayLike<number>,
  fs: number = PREVIEW_FS,
): number[] {
  const out: number[] = new Array(freqsHz.length);
  for (let i = 0; i < freqsHz.length; i++) {
    const f = freqsHz[i];
    let db = 0;
    for (const band of bands) {
      if (band.gain !== 0) {
        db += peakingResponseDb(f, band.frequency, EQ_BAND_Q, band.gain, fs);
      }
    }
    out[i] = db;
  }
  return out;
}

/** `count` log-spaced frequencies from `min`..`max` Hz — the x-axis for the
 *  response curve. */
export function logFreqAxis(count: number, min = 20, max = 20000): number[] {
  const out: number[] = new Array(count);
  const logMin = Math.log10(min);
  const logMax = Math.log10(max);
  const span = logMax - logMin;
  for (let i = 0; i < count; i++) {
    const t = count === 1 ? 0 : i / (count - 1);
    out[i] = Math.pow(10, logMin + t * span);
  }
  return out;
}
