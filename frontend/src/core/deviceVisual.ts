/**
 * Device-class + OS parsed from a device's display name, so the Speakers
 * picker can draw an at-a-glance icon (a PC/phone/tablet/TV shell with the
 * OS logo inside) instead of an emoji that renders as tofu on many browsers.
 *
 * Names come from `defaultDeviceName()` ("Windows PC · Firefox",
 * "Android phone · Chrome", "iPad · Safari", "Mac · Chrome", "Linux · Firefox")
 * or a custom operator label ("Living Room TV", "Kitchen Speaker"). Pure +
 * unit-tested — no DOM, no `navigator`.
 */

export type DeviceClass =
  | "desktop"
  | "phone"
  | "tablet"
  | "tv"
  | "speaker"
  | "unknown";

/** OS whose logo we draw inside the device screen. `apple` covers macOS *and*
 *  iOS (one logo); the device class carries the phone-vs-computer split. */
export type DeviceOs = "windows" | "apple" | "linux" | "android" | null;

export interface DeviceVisual {
  klass: DeviceClass;
  os: DeviceOs;
}

export function parseDeviceVisual(name: string | null | undefined): DeviceVisual {
  const s = (name ?? "").toLowerCase();

  let os: DeviceOs = null;
  if (/windows|🪟/.test(s)) os = "windows";
  else if (/android|🤖/.test(s)) os = "android";
  else if (/iphone|ipad|ipod|\bios\b|macintosh|macbook|imac|\bmac\b|🍎/.test(s))
    os = "apple";
  else if (/linux|x11|ubuntu|debian|fedora|\barch\b|🐧/.test(s)) os = "linux";

  // Class is checked tv → tablet → phone → speaker → computer so the more
  // specific keyword wins ("Android tablet" is a tablet, not a phone).
  let klass: DeviceClass;
  if (/\btv\b|television|chromecast|fire ?stick|fire ?tv|webos|tizen|roku|\bcast\b/.test(s))
    klass = "tv";
  else if (/ipad|tablet/.test(s)) klass = "tablet";
  else if (/iphone|\bphone\b|\bmobile\b/.test(s)) klass = "phone";
  else if (/speaker|sonos|homepod|\becho\b/.test(s)) klass = "speaker";
  else if (
    /\bpc\b|desktop|computer|laptop|macbook|imac|windows|linux|\bmac\b/.test(s)
  )
    klass = "desktop";
  // A bare OS keyword with no class hint: mobile OSes default to a phone, the
  // rest fall through to the generic shell.
  else if (os === "android" || os === "apple") klass = "phone";
  else klass = "unknown";

  return { klass, os };
}

/** Lead segment of a "<platform> · <browser>" name — the part the icon now
 *  conveys, so we can drop it from the visible text. */
const PLATFORM_LEAD =
  /^(windows pc|windows|android(?: phone| tablet)?|iphone|ipad|ipod|macbook|imac|macintosh|mac ?os ?x?|mac|linux|chromebook|browser)$/i;

/**
 * The text to show beside the device icon. When the name is the auto-generated
 * "<platform> · <browser>", strip the platform half (the icon already shows it)
 * and keep just the browser — so "Windows PC · Firefox" reads as a Windows-PC
 * icon + "Firefox". Custom names ("Living Room TV") and platform-only names
 * pass through unchanged. Empty → "This device".
 */
export function deviceDisplayName(name: string | null | undefined): string {
  const n = (name ?? "").trim();
  if (n === "") return "This device";
  const m = n.match(/^(.*?)\s*·\s*(.+)$/);
  if (m !== null && PLATFORM_LEAD.test(m[1].trim())) return m[2].trim();
  return n;
}
