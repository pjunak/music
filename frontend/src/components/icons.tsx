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

/* --- glyphs that replaced emoji used as UI icons -------------------- */

export function SettingsIcon(props: IconProps) {
  // Cog — header "manage modes" + popover gear (was ⚙).
  return (
    <svg {...baseProps} {...props}>
      <circle cx="12" cy="12" r="3" {...stroke} />
      <path
        d="M19.4 15a1.65 1.65 0 00.33 1.82l.06.06a2 2 0 11-2.83 2.83l-.06-.06a1.65 1.65 0 00-1.82-.33 1.65 1.65 0 00-1 1.51V21a2 2 0 01-4 0v-.09A1.65 1.65 0 009 19.4a1.65 1.65 0 00-1.82.33l-.06.06a2 2 0 11-2.83-2.83l.06-.06a1.65 1.65 0 00.33-1.82 1.65 1.65 0 00-1.51-1H3a2 2 0 010-4h.09A1.65 1.65 0 004.6 9a1.65 1.65 0 00-.33-1.82l-.06-.06a2 2 0 112.83-2.83l.06.06A1.65 1.65 0 009 4.6a1.65 1.65 0 001-1.51V3a2 2 0 014 0v.09a1.65 1.65 0 001 1.51 1.65 1.65 0 001.82-.33l.06-.06a2 2 0 112.83 2.83l-.06.06a1.65 1.65 0 00-.33 1.82V9a1.65 1.65 0 001.51 1H21a2 2 0 010 4h-.09a1.65 1.65 0 00-1.51 1z"
        {...stroke}
      />
    </svg>
  );
}

export function MusicNoteIcon(props: IconProps) {
  // Music root toggle (was 🎵).
  return (
    <svg {...baseProps} {...props}>
      <path d="M9 18V5l12-2v13" {...stroke} />
      <circle cx="6" cy="18" r="3" {...stroke} />
      <circle cx="18" cy="16" r="3" {...stroke} />
    </svg>
  );
}

export function SearchIcon(props: IconProps) {
  // Library search (was 🔍).
  return (
    <svg {...baseProps} {...props}>
      <circle cx="11" cy="11" r="7" {...stroke} />
      <path d="M21 21l-4.35-4.35" {...stroke} />
    </svg>
  );
}

export function RescanIcon(props: IconProps) {
  // Two circular arrows — Rescan / Refresh (was ↻).
  return (
    <svg {...baseProps} {...props}>
      <path d="M21 3v6h-6" {...stroke} />
      <path d="M3 12a9 9 0 0115-6.7L21 9" {...stroke} />
      <path d="M3 21v-6h6" {...stroke} />
      <path d="M21 12a9 9 0 01-15 6.7L3 15" {...stroke} />
    </svg>
  );
}

export function MoveIcon(props: IconProps) {
  // Folder + out-arrow — "Move…" a folder/selection (was ↪).
  return (
    <svg {...baseProps} {...props}>
      <path d="M4 7a2 2 0 012-2h3l2 2h4a2 2 0 012 2v2" {...stroke} />
      <path d="M4 9v9a1 1 0 001 1h6" {...stroke} />
      <path d="M14 18h7M18 15l3 3-3 3" {...stroke} />
    </svg>
  );
}

export function ImportIcon(props: IconProps) {
  // Down-into-tray — Upload/Import (was ⬇).
  return (
    <svg {...baseProps} {...props}>
      <path d="M12 3v12M7 10l5 5 5-5" {...stroke} />
      <path d="M5 21h14" {...stroke} />
    </svg>
  );
}

export function ModeIcon(props: IconProps) {
  // Stylised mask — the active "mode/theme" (was 🎭).
  return (
    <svg {...baseProps} {...props}>
      <path d="M4 5h16v5a8 8 0 01-16 0V5z" {...stroke} />
      <circle cx="9" cy="9" r="0.6" fill="currentColor" />
      <circle cx="15" cy="9" r="0.6" fill="currentColor" />
      <path d="M9 13c1.2 1 4.8 1 6 0" {...stroke} />
    </svg>
  );
}

export function TagIcon(props: IconProps) {
  // Tag — metadata / nickname (was 🏷).
  return (
    <svg {...baseProps} {...props}>
      <path d="M3 12V4a1 1 0 011-1h8l9 9-9 9z" {...stroke} />
      <circle cx="7.5" cy="7.5" r="1.2" fill="currentColor" />
    </svg>
  );
}

export function CheckIcon(props: IconProps) {
  return (
    <svg {...baseProps} {...props}>
      <path d="M5 12l5 5L20 6" {...stroke} />
    </svg>
  );
}

export function InfoIcon(props: IconProps) {
  return (
    <svg {...baseProps} {...props}>
      <circle cx="12" cy="12" r="9" {...stroke} />
      <path d="M12 11v5" {...stroke} />
      <circle cx="12" cy="8" r="0.7" fill="currentColor" />
    </svg>
  );
}

export function WarnIcon(props: IconProps) {
  return (
    <svg {...baseProps} {...props}>
      <path d="M12 3.5L21 19H3z" {...stroke} />
      <path d="M12 10v4" {...stroke} />
      <circle cx="12" cy="16.5" r="0.7" fill="currentColor" />
    </svg>
  );
}

export function ErrorIcon(props: IconProps) {
  return (
    <svg {...baseProps} {...props}>
      <circle cx="12" cy="12" r="9" {...stroke} />
      <path d="M9 9l6 6M15 9l-6 6" {...stroke} />
    </svg>
  );
}

export function SparkleIcon(props: IconProps) {
  // "Clean up" affordance — a large four-point sparkle with a small echo.
  return (
    <svg {...baseProps} {...props}>
      <path d="M10 3l1.7 4.8L16.5 9.5l-4.8 1.7L10 16l-1.7-4.8L3.5 9.5l4.8-1.7z" fill="currentColor" />
      <path d="M18 13l1 2.7 2.7 1-2.7 1-1 2.7-1-2.7-2.7-1 2.7-1z" fill="currentColor" />
    </svg>
  );
}
