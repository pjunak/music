import { useId, useRef } from "react";
import type { PointerEvent as ReactPointerEvent } from "react";

import {
  clamp,
  handleSliderKey,
  normToValue,
  roundToStep,
  valueToNorm,
} from "./sliderControl";
import type { Scale } from "./sliderControl";

/** A rotary knob — vertical drag to turn, like a hardware pot. Renders an SVG
 *  gauge (270° sweep) with a value arc + indicator. Fully keyboard-driven
 *  (`role="slider"`), so the app's global shortcuts leave it alone
 *  (`isInteractiveTarget` treats role=slider as interactive). */

interface Props {
  value: number;
  min: number;
  max: number;
  step: number;
  onChange: (next: number) => void;
  label: string;
  /** Formats the live readout under the knob. */
  format: (v: number) => string;
  /** Double-click resets here, when provided. */
  def?: number;
  /** Log scale suits wide frequency ranges (even feel per octave). */
  scale?: Scale;
  /** Diameter in px. Default 46. */
  size?: number;
  disabled?: boolean;
}

const START_DEG = -135;
const SWEEP_DEG = 270;
// Pixels of vertical drag to traverse the full range (Shift = 5× finer).
const DRAG_FULL_PX = 160;

function polar(cx: number, cy: number, r: number, deg: number): [number, number] {
  const a = ((deg - 90) * Math.PI) / 180;
  return [cx + r * Math.cos(a), cy + r * Math.sin(a)];
}

function arc(cx: number, cy: number, r: number, startDeg: number, endDeg: number): string {
  const [sx, sy] = polar(cx, cy, r, startDeg);
  const [ex, ey] = polar(cx, cy, r, endDeg);
  const large = Math.abs(endDeg - startDeg) > 180 ? 1 : 0;
  return `M ${sx} ${sy} A ${r} ${r} 0 ${large} 1 ${ex} ${ey}`;
}

export function Knob({
  value,
  min,
  max,
  step,
  onChange,
  label,
  format,
  def,
  scale = "linear",
  size = 46,
  disabled = false,
}: Props) {
  const rawId = useId();
  const labelId = `${rawId}-l`;
  // useId yields colons (":r0:") which break SVG `url(#…)` refs — strip them.
  const capId = `knobcap-${rawId.replace(/:/g, "")}`;
  const drag = useRef<{ startY: number; startNorm: number } | null>(null);

  const t = valueToNorm(value, min, max, scale);
  const angle = START_DEG + t * SWEEP_DEG;
  const cx = 50;
  const cy = 50;
  const gaugeR = 40;
  const [ix, iy] = polar(cx, cy, 30, angle);
  const [hx, hy] = polar(cx, cy, 13, angle);

  function commit(norm: number) {
    const raw = normToValue(norm, min, max, scale);
    onChange(clamp(roundToStep(raw, step, min), min, max));
  }

  function onPointerDown(e: ReactPointerEvent<HTMLDivElement>) {
    if (disabled) return;
    e.preventDefault();
    (e.currentTarget as HTMLElement).focus();
    e.currentTarget.setPointerCapture(e.pointerId);
    drag.current = { startY: e.clientY, startNorm: valueToNorm(value, min, max, scale) };
  }

  function onPointerMove(e: ReactPointerEvent<HTMLDivElement>) {
    if (!drag.current) return;
    const dy = drag.current.startY - e.clientY; // up = increase
    const sensitivity = e.shiftKey ? DRAG_FULL_PX * 5 : DRAG_FULL_PX;
    commit(drag.current.startNorm + dy / sensitivity);
  }

  function endDrag(e: ReactPointerEvent<HTMLDivElement>) {
    if (!drag.current) return;
    drag.current = null;
    if (e.currentTarget.hasPointerCapture(e.pointerId)) {
      e.currentTarget.releasePointerCapture(e.pointerId);
    }
  }

  return (
    <div className={`knob${disabled ? " knob-disabled" : ""}`}>
      <div
        className="knob-dial"
        role="slider"
        tabIndex={disabled ? -1 : 0}
        aria-labelledby={labelId}
        aria-valuemin={min}
        aria-valuemax={max}
        aria-valuenow={Number(value.toFixed(4))}
        aria-valuetext={format(value)}
        aria-disabled={disabled || undefined}
        style={{ width: size, height: size }}
        onPointerDown={onPointerDown}
        onPointerMove={onPointerMove}
        onPointerUp={endDrag}
        onPointerCancel={endDrag}
        onDoubleClick={() => def !== undefined && !disabled && onChange(def)}
        onKeyDown={(e) =>
          !disabled && handleSliderKey(e, { value, min, max, step, onChange })
        }
      >
        <svg viewBox="0 0 100 100" aria-hidden="true">
          <defs>
            <radialGradient id={capId} cx="50%" cy="36%" r="68%">
              <stop offset="0%" stopColor="#333944" />
              <stop offset="100%" stopColor="#15181d" />
            </radialGradient>
          </defs>
          <path className="knob-track" d={arc(cx, cy, gaugeR, START_DEG, START_DEG + SWEEP_DEG)} />
          <path className="knob-value" d={arc(cx, cy, gaugeR, START_DEG, angle)} />
          <circle className="knob-body" cx={cx} cy={cy} r={28} fill={`url(#${capId})`} />
          <line className="knob-indicator" x1={hx} y1={hy} x2={ix} y2={iy} />
        </svg>
      </div>
      <span className="knob-label" id={labelId}>
        {label}
      </span>
      <span className="knob-readout">{format(value)}</span>
    </div>
  );
}
