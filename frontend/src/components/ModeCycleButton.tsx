import type { ReactNode } from "react";

/** One state in a cycling control (repeat / shuffle). */
export interface ModeOption<T extends string> {
  value: T;
  /** Glyph shown while this mode is current. */
  icon: ReactNode;
  /** Whether this mode is "engaged" — drives the accent highlight. */
  active: boolean;
  /** Descriptive legend used as both tooltip and accessible name. Should
   *  read the current state AND what a click does, e.g.
   *  "Repeat off — click to repeat the current track". */
  legend: string;
}

interface Props<T extends string> {
  /** Ordered cycle; a click advances to the next entry, wrapping at the end. */
  options: ModeOption<T>[];
  current: T;
  onCycle: (next: T) => void;
  /** Read-only (host / guest display): the control stays visible and keeps
   *  showing its active state, but isn't interactive. Mirrors the
   *  `VolumeControl` readOnly contract used elsewhere in the footer. */
  readOnly?: boolean;
  /** Appended to the tooltip when readOnly to explain why it's locked. */
  readOnlyHint?: string;
}

/** A single icon control that cycles through a small set of modes on click.
 *
 *  Written as a function declaration (not an arrow) so the `<T extends string>`
 *  generic doesn't collide with JSX parsing in this `.tsx` file. */
export function ModeCycleButton<T extends string>({
  options,
  current,
  onCycle,
  readOnly = false,
  readOnlyHint,
}: Props<T>) {
  const idx = Math.max(
    0,
    options.findIndex((o) => o.value === current),
  );
  const opt = options[idx];
  const next = options[(idx + 1) % options.length];

  const className = ["mode-cycle", opt.active ? "mode-cycle-active" : null]
    .filter(Boolean)
    .join(" ");
  const glyph = (
    <span className="icon-button-glyph" aria-hidden="true">
      {opt.icon}
    </span>
  );

  if (readOnly) {
    const title = readOnlyHint ? `${opt.legend} · ${readOnlyHint}` : opt.legend;
    return (
      <span
        className={`${className} mode-cycle-readonly`}
        title={title}
        role="img"
        aria-label={opt.legend}
      >
        {glyph}
      </span>
    );
  }

  return (
    <button
      type="button"
      className={className}
      onClick={() => onCycle(next.value)}
      title={opt.legend}
      aria-label={opt.legend}
      aria-pressed={opt.active}
    >
      {glyph}
    </button>
  );
}
