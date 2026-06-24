/**
 * Shorten a device name by swapping the verbose OS name for its emoji
 * ("Windows PC · Edge" → "🪟 PC · Edge", "Mac · Safari" → "🍎 · Safari").
 * Mirrors the Baton Android client so both remotes read the same. Names without an
 * OS keyword (a custom "Living Room TV", a phone model, …) pass through untouched.
 */
export function shortDeviceLabel(name: string): string {
  return name
    .replace(/windows/gi, "🪟")
    .replace(/macintosh|mac os x/gi, "🍎")
    .replace(/\bmac\b/gi, "🍎")
    .replace(/\blinux\b|x11/gi, "🐧")
    .replace(/android/gi, "🤖")
    .replace(/\biphone\b|\bipad\b/gi, "🍎")
    .trim();
}
