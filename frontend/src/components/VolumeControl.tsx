import type { CSSProperties, ReactNode } from "react";

import { VolumeIcon } from "./icons";

/** Volume slider with painted level fill, value readout, and icon.
 *
 *  Replaces three near-identical bespoke implementations (NowPlayingBar,
 *  ControlsView, SoundboardSection) so they look and behave the same.
 *
 *  The painted level fill uses the seek-bar's "played vs unplayed" cue
 *  to make the volume reading pre-attentive: Firefox uses native
 *  `::-moz-range-progress`, webkit uses a gradient driven by the inline
 *  `--volume-pct` custom property set here. */

interface Props {
  /** Current volume in 0–1. */
  value: number;
  /** Receives the new volume in 0–1. */
  onChange: (next: number) => void;
  /** Visible hint label / tooltip (also feeds aria-label). */
  label?: string;
  /** Show a speaker icon to the left of the slider. Defaults to true. */
  showIcon?: boolean;
  /** Show the percentage value to the right. Defaults to true. */
  showPercent?: boolean;
  /** Optional content rendered before the icon — e.g. a "SFX volume" hint. */
  prefix?: ReactNode;
  /** Optional className passed to the wrapping <label>, for layout overrides. */
  className?: string;
}

export function VolumeControl({
  value,
  onChange,
  label = "Volume",
  showIcon = true,
  showPercent = true,
  prefix,
  className,
}: Props) {
  const composed = ["volume-row", className].filter(Boolean).join(" ");
  return (
    <label className={composed} title={label}>
      {prefix !== undefined ? (
        <span className="volume-row-prefix muted small">{prefix}</span>
      ) : null}
      {showIcon ? <VolumeIcon className="volume-row-icon" /> : null}
      <input
        className="volume-slider"
        type="range"
        min={0}
        max={1}
        step={0.01}
        value={value}
        onChange={(e) => onChange(parseFloat(e.target.value))}
        aria-label={label}
        style={{ "--volume-pct": `${value * 100}%` } as CSSProperties}
      />
      {showPercent ? (
        <span className="volume-row-pct muted small">
          {Math.round(value * 100)}%
        </span>
      ) : null}
    </label>
  );
}
