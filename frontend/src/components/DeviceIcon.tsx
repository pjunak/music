import type { SVGProps } from "react";

import {
  type DeviceOs,
  parseDeviceVisual,
} from "@/core/deviceVisual";

/** An at-a-glance device icon for the Speakers picker: a device shell
 *  (monitor / phone / tablet / TV / speaker) with the OS logo drawn inside
 *  the screen (PC + Windows panes, phone + Android robot, …). Replaces the
 *  old emoji label, which rendered as a tofu box on many browsers — the same
 *  reason the transport glyphs became SVGs (see icons.tsx).
 *
 *  Authored in a 24×24 viewBox; renders at 1em so font-size drives the size
 *  and it inherits `currentColor`. The OS logo is composed from a separate
 *  24-box, scaled down and centred into the shell's screen area, so each logo
 *  is authored once and reused across shells. */

const base = {
  width: "1em",
  height: "1em",
  viewBox: "0 0 24 24",
  xmlns: "http://www.w3.org/2000/svg",
  "aria-hidden": true,
} as const;

// Device shells are outlines; OS logos are solid fills (so scaling them down
// doesn't thin a stroke). Keep shells a touch lighter than 2 so the bezel
// doesn't crowd the logo at small sizes.
const shell = {
  fill: "none",
  stroke: "currentColor",
  strokeWidth: 1.7,
  strokeLinecap: "round" as const,
  strokeLinejoin: "round" as const,
};

/** Place a logo authored in its own 24-box at `(cx,cy)` scaled to `size`
 *  (both in the outer viewBox's units). */
function logoTransform(cx: number, cy: number, size: number): string {
  return `translate(${cx} ${cy}) scale(${size / 24}) translate(-12 -12)`;
}

function OsLogo({
  os,
  cx,
  cy,
  size,
}: {
  os: DeviceOs;
  cx: number;
  cy: number;
  size: number;
}) {
  if (os === null) return null;
  const t = logoTransform(cx, cy, size);
  switch (os) {
    case "windows":
      // Four panes — the modern Windows mark. Reads cleanly even at ~8px.
      return (
        <g transform={t} fill="currentColor">
          <rect x="2.5" y="2.5" width="8.5" height="8.5" rx="0.6" />
          <rect x="13" y="2.5" width="8.5" height="8.5" rx="0.6" />
          <rect x="2.5" y="13" width="8.5" height="8.5" rx="0.6" />
          <rect x="13" y="13" width="8.5" height="8.5" rx="0.6" />
        </g>
      );
    case "apple":
      // Apple silhouette + leaf. The bite is the concave curve on the right.
      return (
        <g transform={t} fill="currentColor">
          <path d="M17.9 12.6c-.02-2.7 2.2-4 2.3-4.06-1.25-1.83-3.2-2.08-3.9-2.1-1.66-.18-3.24.97-4.08.97-.85 0-2.22-.95-3.65-.92-1.88.03-3.6 1.09-4.57 2.78-1.95 3.38-.5 8.4 1.4 11.15.93 1.35 2.04 2.86 3.5 2.8 1.4-.05 1.94-.9 3.64-.9 1.7 0 2.18.9 3.66.87 1.51-.03 2.47-1.37 3.39-2.73.43-.62.78-1.27 1.06-1.95-2.78-1.06-3.27-4.96-3.28-5.03z" />
          <path d="M15 4.9c.77-.94 1.3-2.24 1.15-3.55-1.12.05-2.47.75-3.27 1.68-.72.82-1.36 2.14-1.19 3.4 1.24.1 2.5-.63 3.31-1.53z" />
        </g>
      );
    case "android":
      // Robot head: domed top, two antennae, two eyes (knocked out with
      // even-odd so they show the screen behind, theme-independently).
      return (
        <g transform={t}>
          <path
            d="M8.4 7.6 6.7 4.6 M15.6 7.6 17.3 4.6"
            fill="none"
            stroke="currentColor"
            strokeWidth="2.6"
            strokeLinecap="round"
          />
          <path
            fillRule="evenodd"
            fill="currentColor"
            d="M4.5 13.5a7.5 7.5 0 0 1 15 0v.6h-15v-.6zM9.6 10.6a1.15 1.15 0 1 0 0 .02zM14.4 10.6a1.15 1.15 0 1 0 0 .02z"
          />
        </g>
      );
    case "linux":
      // Simplified Tux — body + head silhouette with a knocked-out belly and
      // eyes (even-odd), plus a small beak. Not the full mascot, but reads as
      // "penguin → Linux" next to the other marks.
      return (
        <g transform={t}>
          <path
            fillRule="evenodd"
            fill="currentColor"
            d="M12 2.6c-2.5 0-4.2 2-4.2 4.8 0 1.4-.45 2.1-1 3-.8 1.3-1.9 2.7-1.9 4.5 0 1.1.7 1.9 1.6 2.4-.3.7-.45 1.4-.45 2 0 1.9 2.7 3.3 6 3.3s6-1.4 6-3.3c0-.6-.15-1.3-.45-2 .9-.5 1.6-1.3 1.6-2.4 0-1.8-1.1-3.2-1.9-4.5-.55-.9-1-1.6-1-3C16.2 4.6 14.5 2.6 12 2.6Zm-1.7 5.2a1 1 0 1 1 0 .02zm3.4 0a1 1 0 1 1 0 .02zM12 8.7l1.4 1.5c0 .6-.7 1-1.4 1s-1.4-.4-1.4-1L12 8.7Z"
          />
          <path
            fill="currentColor"
            fillRule="evenodd"
            d="M12 13.4c-2 0-3.5 1.9-3.5 4.3 0 2 1.6 3.2 3.5 3.2s3.5-1.2 3.5-3.2c0-2.4-1.5-4.3-3.5-4.3Zm0 1.7c1 0 1.8 1.1 1.8 2.6s-.8 2-1.8 2-1.8-.5-1.8-2 .8-2.6 1.8-2.6Z"
          />
        </g>
      );
  }
}

/** Phone / tablet differ only in proportions; share one shell. */
function MobileShell({ os, wide }: { os: DeviceOs; wide: boolean }) {
  const x = wide ? 4 : 6.5;
  const w = wide ? 16 : 11;
  return (
    <>
      <rect x={x} y="2.2" width={w} height="19.6" rx={wide ? 1.8 : 2.4} {...shell} />
      <OsLogo os={os} cx={12} cy={11} size={wide ? 9 : 6.6} />
    </>
  );
}

function MonitorShell({ os }: { os: DeviceOs }) {
  return (
    <>
      <rect x="2.4" y="3.3" width="19.2" height="13" rx="1.6" {...shell} />
      <path d="M12 16.3v2.7M8.5 19.6h7" {...shell} />
      <OsLogo os={os} cx={12} cy={9.8} size={8.6} />
    </>
  );
}

function TvShell() {
  // Wide screen on two splayed legs — distinct from the monitor's pedestal.
  // No OS logo: a TV reads as a TV (per the room-display use case).
  return (
    <>
      <rect x="1.8" y="3.6" width="20.4" height="13.4" rx="1.4" {...shell} />
      <path d="M9.4 17l-1.6 3.4M14.6 17l1.6 3.4" {...shell} />
    </>
  );
}

function SpeakerShell() {
  // Headless audio appliance — a box with two drivers; no OS logo.
  return (
    <>
      <rect x="6.5" y="2.4" width="11" height="19.2" rx="2" {...shell} />
      <circle cx="12" cy="7.6" r="1.7" {...shell} />
      <circle cx="12" cy="14.8" r="3.1" {...shell} />
    </>
  );
}

// Omit the SVG `name` attribute (unused here) so our device-name prop can be
// nullable without clashing with it.
interface DeviceIconProps extends Omit<SVGProps<SVGSVGElement>, "name"> {
  /** Device display name (server-registered or custom) to derive the icon. */
  name: string | null | undefined;
}

export function DeviceIcon({ name, ...props }: DeviceIconProps) {
  const { klass, os } = parseDeviceVisual(name);
  return (
    <svg {...base} {...props}>
      {klass === "tv" ? (
        <TvShell />
      ) : klass === "speaker" ? (
        <SpeakerShell />
      ) : klass === "phone" ? (
        <MobileShell os={os} wide={false} />
      ) : klass === "tablet" ? (
        <MobileShell os={os} wide={true} />
      ) : (
        // desktop + unknown → a monitor (the unknown device is most likely a
        // computer in this single-operator setup); unknown has os === null.
        <MonitorShell os={os} />
      )}
    </svg>
  );
}
