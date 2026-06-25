/** Shared helpers for the custom audio-control widgets (Knob, Fader): value
 *  clamping, step quantisation, linear/log normalisation, and a keyboard
 *  handler that gives every control the standard ARIA-slider key bindings. */
import type { KeyboardEvent } from "react";

export type Scale = "linear" | "log";

export function clamp(v: number, min: number, max: number): number {
  return Math.max(min, Math.min(max, v));
}

export function roundToStep(v: number, step: number, min: number): number {
  if (step <= 0) return v;
  return min + Math.round((v - min) / step) * step;
}

/** Value → 0..1 knob/fader position. Log scale suits wide frequency ranges so
 *  the control feels even per octave rather than bunched at the low end. */
export function valueToNorm(
  value: number,
  min: number,
  max: number,
  scale: Scale = "linear",
): number {
  if (max === min) return 0;
  if (scale === "log") {
    const lo = Math.log(Math.max(min, 1e-6));
    const hi = Math.log(Math.max(max, 1e-6));
    return clamp((Math.log(Math.max(value, 1e-6)) - lo) / (hi - lo), 0, 1);
  }
  return clamp((value - min) / (max - min), 0, 1);
}

export function normToValue(
  t: number,
  min: number,
  max: number,
  scale: Scale = "linear",
): number {
  const c = clamp(t, 0, 1);
  if (scale === "log") {
    const lo = Math.log(Math.max(min, 1e-6));
    const hi = Math.log(Math.max(max, 1e-6));
    return Math.exp(lo + c * (hi - lo));
  }
  return min + c * (max - min);
}

interface KeyOpts {
  value: number;
  min: number;
  max: number;
  step: number;
  onChange: (v: number) => void;
}

/** Standard slider keys: arrows nudge by `step`, PageUp/Down by a tenth of the
 *  range, Home/End jump to the ends. Returns true if it handled the key (so the
 *  caller can `preventDefault` is already done here). */
export function handleSliderKey(e: KeyboardEvent, opts: KeyOpts): boolean {
  const { value, min, max, step, onChange } = opts;
  const big = Math.max(step, (max - min) / 10);
  let next: number;
  switch (e.key) {
    case "ArrowUp":
    case "ArrowRight":
      next = value + step;
      break;
    case "ArrowDown":
    case "ArrowLeft":
      next = value - step;
      break;
    case "PageUp":
      next = value + big;
      break;
    case "PageDown":
      next = value - big;
      break;
    case "Home":
      next = min;
      break;
    case "End":
      next = max;
      break;
    default:
      return false;
  }
  e.preventDefault();
  onChange(clamp(roundToStep(next, step, min), min, max));
  return true;
}
