import type { SVGProps } from "react";

/** Inline SVG icon set.
 *
 *  Icons render at 1em × 1em and inherit `currentColor`, so they pick up the
 *  surrounding font-size and text colour automatically. This replaces the old
 *  unicode glyph buttons (▶ ⏸ ⏭ etc.) that rendered inconsistently across
 *  Firefox / Chrome / mobile because of fallback fonts and emoji-presentation
 *  selectors.
 *
 *  Conventions:
 *  - 24×24 viewBox so a stroke width of 2 hits a half-pixel grid at 16px / 24px.
 *  - Solid icons (Play, Pause, Trash, Lightning) use `fill="currentColor"`,
 *    no stroke. Outline icons use `stroke="currentColor"` with `fill="none"`.
 *  - All icons are pure components — they accept any SVG props for overrides
 *    (`className`, `aria-hidden`, etc.).
 */

type IconProps = SVGProps<SVGSVGElement>;

const baseProps = {
  width: "1em",
  height: "1em",
  viewBox: "0 0 24 24",
  xmlns: "http://www.w3.org/2000/svg",
  "aria-hidden": true,
} as const;

const stroke = {
  fill: "none",
  stroke: "currentColor",
  strokeWidth: 2,
  strokeLinecap: "round" as const,
  strokeLinejoin: "round" as const,
};

export function PlayIcon(props: IconProps) {
  // Triangle vertices chosen so the centroid sits at x=12 (centre of the
  // 24-unit viewBox), so the icon doesn't look left-shifted when its
  // bounding box is centred — the visual fix the unicode ▶ glyph needed.
  return (
    <svg {...baseProps} {...props}>
      <path d="M8 4.5v15l12-7.5z" fill="currentColor" />
    </svg>
  );
}

export function PauseIcon(props: IconProps) {
  return (
    <svg {...baseProps} {...props}>
      <rect x="6" y="4.5" width="3.5" height="15" rx="1" fill="currentColor" />
      <rect x="14.5" y="4.5" width="3.5" height="15" rx="1" fill="currentColor" />
    </svg>
  );
}

export function SkipPrevIcon(props: IconProps) {
  return (
    <svg {...baseProps} {...props}>
      <path d="M6 5v14M19 5v14l-11-7z" {...stroke} />
    </svg>
  );
}

export function SkipNextIcon(props: IconProps) {
  return (
    <svg {...baseProps} {...props}>
      <path d="M18 5v14M5 5v14l11-7z" {...stroke} />
    </svg>
  );
}

export function PlusIcon(props: IconProps) {
  return (
    <svg {...baseProps} {...props}>
      <path d="M12 5v14M5 12h14" {...stroke} />
    </svg>
  );
}

export function XIcon(props: IconProps) {
  return (
    <svg {...baseProps} {...props}>
      <path d="M6 6l12 12M18 6l-12 12" {...stroke} />
    </svg>
  );
}

export function TrashIcon(props: IconProps) {
  return (
    <svg {...baseProps} {...props}>
      <path
        d="M5 7h14M10 7V5a1 1 0 011-1h2a1 1 0 011 1v2M7 7l1 12a2 2 0 002 2h4a2 2 0 002-2l1-12M10 11v6M14 11v6"
        {...stroke}
      />
    </svg>
  );
}

export function EditIcon(props: IconProps) {
  return (
    <svg {...baseProps} {...props}>
      <path
        d="M4 20h4l11-11-4-4L4 16v4zM14 6l4 4"
        {...stroke}
      />
    </svg>
  );
}

export function ArrowUpIcon(props: IconProps) {
  return (
    <svg {...baseProps} {...props}>
      <path d="M12 19V5M5 12l7-7 7 7" {...stroke} />
    </svg>
  );
}

export function ArrowDownIcon(props: IconProps) {
  return (
    <svg {...baseProps} {...props}>
      <path d="M12 5v14M5 12l7 7 7-7" {...stroke} />
    </svg>
  );
}

export function LightningIcon(props: IconProps) {
  return (
    <svg {...baseProps} {...props}>
      <path d="M13 2L4 14h7l-1 8 9-12h-7l1-8z" fill="currentColor" />
    </svg>
  );
}

export function FolderClosedIcon(props: IconProps) {
  return (
    <svg {...baseProps} {...props}>
      <path
        d="M3 7a2 2 0 012-2h4l2 2h8a2 2 0 012 2v9a2 2 0 01-2 2H5a2 2 0 01-2-2V7z"
        {...stroke}
      />
    </svg>
  );
}

export function FolderOpenIcon(props: IconProps) {
  return (
    <svg {...baseProps} {...props}>
      <path
        d="M3 7a2 2 0 012-2h4l2 2h8a2 2 0 012 2v1H3V7zM3 9h18l-2 8a2 2 0 01-2 2H5a2 2 0 01-2-2V9z"
        {...stroke}
      />
    </svg>
  );
}

export function ChevronRightIcon(props: IconProps) {
  return (
    <svg {...baseProps} {...props}>
      <path d="M9 6l6 6-6 6" {...stroke} />
    </svg>
  );
}

export function ChevronDownIcon(props: IconProps) {
  return (
    <svg {...baseProps} {...props}>
      <path d="M6 9l6 6 6-6" {...stroke} />
    </svg>
  );
}

export function RepeatIcon(props: IconProps) {
  // Two-thirds loop with arrowheads at the breaks — the universal "repeat"
  // glyph. Used for both "off" (muted) and "repeat queue" (accent) states;
  // the colour + legend carry which one is active.
  return (
    <svg {...baseProps} {...props}>
      <path d="M17 2l4 4-4 4" {...stroke} />
      <path d="M3 11V9a4 4 0 014-4h14" {...stroke} />
      <path d="M7 22l-4-4 4-4" {...stroke} />
      <path d="M21 13v2a4 4 0 01-4 4H3" {...stroke} />
    </svg>
  );
}

export function RepeatOneIcon(props: IconProps) {
  // Repeat loop with a "1" in the middle — repeat the current track only.
  return (
    <svg {...baseProps} {...props}>
      <path d="M17 2l4 4-4 4" {...stroke} />
      <path d="M3 11V9a4 4 0 014-4h14" {...stroke} />
      <path d="M7 22l-4-4 4-4" {...stroke} />
      <path d="M21 13v2a4 4 0 01-4 4H3" {...stroke} />
      <path d="M11 9.5l1.5-1V15" {...stroke} />
    </svg>
  );
}

export function ShuffleIcon(props: IconProps) {
  // Crossing arrows — the universal "shuffle" glyph.
  return (
    <svg {...baseProps} {...props}>
      <path d="M16 3h5v5" {...stroke} />
      <path d="M4 20L21 3" {...stroke} />
      <path d="M21 16v5h-5" {...stroke} />
      <path d="M15 15l6 6" {...stroke} />
      <path d="M4 4l5 5" {...stroke} />
    </svg>
  );
}

export function ShuffleWeightedIcon(props: IconProps) {
  // Shuffle arrows with a filled centre dot — "loaded"/weighted random,
  // distinct from plain shuffle at a glance.
  return (
    <svg {...baseProps} {...props}>
      <path d="M16 3h5v5" {...stroke} />
      <path d="M4 20L21 3" {...stroke} />
      <path d="M21 16v5h-5" {...stroke} />
      <path d="M15 15l6 6" {...stroke} />
      <path d="M4 4l5 5" {...stroke} />
      <circle cx="12" cy="12" r="2" fill="currentColor" stroke="none" />
    </svg>
  );
}

export function VolumeIcon(props: IconProps) {
  return (
    <svg {...baseProps} {...props}>
      <path
        d="M4 9h3l5-4v14l-5-4H4V9zM16 8a5 5 0 010 8M19 5a9 9 0 010 14"
        {...stroke}
      />
    </svg>
  );
}

export function HelpIcon(props: IconProps) {
  // Circle + a question-mark glyph. Stroke style matches the rest of the
  // outline icons so it slots into the header alongside ws-status without
  // visually jumping out.
  return (
    <svg {...baseProps} {...props}>
      <circle cx="12" cy="12" r="9" {...stroke} />
      <path d="M9.5 9a2.5 2.5 0 015 0c0 1.5-1.5 2-2 3v0.5" {...stroke} />
      <circle cx="12" cy="17" r="0.5" fill="currentColor" />
    </svg>
  );
}
