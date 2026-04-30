import type { Track, TrackSummary } from "@/core/types";

/** What to render where a track's "title" goes in the UI. The user-entered
 *  `display_title` overrides the tag-derived `title`, which itself falls
 *  back to the file's basename when empty. Centralised so every list /
 *  bar / picker agrees. */
export function trackTitle(t: Track | TrackSummary | null | undefined): string {
  if (!t) return "";
  // TrackSummary doesn't carry display_title — the player listings that use
  // the summary shape can degrade to title/path.
  const dt = (t as Track).display_title;
  if (typeof dt === "string" && dt.trim() !== "") return dt;
  if (t.title && t.title.trim() !== "") return t.title;
  return t.path;
}
