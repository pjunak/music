import { useId, useRef } from "react";
import type { PointerEvent as ReactPointerEvent } from "react";

import { clamp, handleSliderKey, normToValue, roundToStep, valueToNorm } from "./sliderControl";
import type { Scale } from "./sliderControl";

/** A vertical fader — drag the cap (or click anywhere on the throw) to set the
 *  value. `bipolar` draws the fill from the centre detent (for EQ gain, where 0
 *  is the rest position). Keyboard-driven via `role="slider"`. */

interface Props {
  value: number;
  min: number;
  max: number;
  step: number;
  onChange: (next: number) => void;
  /** Caption under the throw (e.g. a band frequency). */
  label?: string;
  /** Readout above the throw + aria-valuetext. */
  format?: (v: number) => string;
  /** Fill from the centre instead of the bottom (EQ boost/cut). */
  bipolar?: boolean;
  scale?: Scale;
  /** Throw height in px. Default 130. */
  height?: number;
  ariaLabel?: string;
  /** Reset target on double-click. */
  def?: number;
  disabled?: boolean;
}

export function Fader({
  value,
  min,
  max,
  step,
  onChange,
  label,
  format,
  bipolar = false,
  scale = "linear",
  height = 130,
  ariaLabel,
  def,
  disabled = false,
}: Props) {
  const labelId = useId();
  const trackRef = useRef<HTMLDivElement | null>(null);
  const dragging = useRef(false);

  const t = valueToNorm(value, min, max, scale);

  // Fill geometry (percent from the bottom).
  const lo = bipolar ? Math.min(t, 0.5) : 0;
  const hi = bipolar ? Math.max(t, 0.5) : t;
  const fillBottom = lo * 100;
  const fillHeight = (hi - lo) * 100;

  function commitFromEvent(e: ReactPointerEvent<HTMLDivElement>) {
    const el = trackRef.current;
    if (!el) return;
    const rect = el.getBoundingClientRect();
    const norm = clamp(1 - (e.clientY - rect.top) / rect.height, 0, 1);
    const raw = normToValue(norm, min, max, scale);
    onChange(clamp(roundToStep(raw, step, min), min, max));
  }

  function onPointerDown(e: ReactPointerEvent<HTMLDivElement>) {
    if (disabled) return;
    e.preventDefault();
    e.currentTarget.focus();
    e.currentTarget.setPointerCapture(e.pointerId);
    dragging.current = true;
    commitFromEvent(e);
  }

  function onPointerMove(e: ReactPointerEvent<HTMLDivElement>) {
    if (dragging.current) commitFromEvent(e);
  }

  function endDrag(e: ReactPointerEvent<HTMLDivElement>) {
    dragging.current = false;
    if (e.currentTarget.hasPointerCapture(e.pointerId)) {
      e.currentTarget.releasePointerCapture(e.pointerId);
    }
  }

  return (
    <div className={`fader${disabled ? " fader-disabled" : ""}`}>
      {format ? <span className="fader-readout">{format(value)}</span> : null}
      <div
        ref={trackRef}
        className={`fader-track${bipolar ? " fader-bipolar" : ""}`}
        role="slider"
        tabIndex={disabled ? -1 : 0}
        aria-label={ariaLabel ?? label}
        aria-labelledby={!ariaLabel && label ? labelId : undefined}
        aria-valuemin={min}
        aria-valuemax={max}
        aria-valuenow={Number(value.toFixed(4))}
        aria-valuetext={format ? format(value) : undefined}
        aria-disabled={disabled || undefined}
        aria-orientation="vertical"
        style={{ height }}
        onPointerDown={onPointerDown}
        onPointerMove={onPointerMove}
        onPointerUp={endDrag}
        onPointerCancel={endDrag}
        onDoubleClick={() => def !== undefined && !disabled && onChange(def)}
        onKeyDown={(e) =>
          !disabled && handleSliderKey(e, { value, min, max, step, onChange })
        }
      >
        {bipolar ? <span className="fader-centerline" aria-hidden="true" /> : null}
        <span
          className="fader-fill"
          aria-hidden="true"
          style={{ bottom: `${fillBottom}%`, height: `${fillHeight}%` }}
        />
        <span
          className="fader-thumb"
          aria-hidden="true"
          style={{ top: `${(1 - t) * 100}%` }}
        />
      </div>
      {label ? (
        <span className="fader-label" id={labelId}>
          {label}
        </span>
      ) : null}
    </div>
  );
}
