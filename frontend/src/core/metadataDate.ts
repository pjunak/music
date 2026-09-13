import type { Track } from "./types";

/** Preserve source precision; these are calendar dates, never instants. */
export function validMetadataDate(value: string): boolean {
  const match = /^(\d{4})(?:-(\d{2})(?:-(\d{2}))?)?$/.exec(value);
  if (!match) return false;
  const year = Number(match[1]);
  if (year === 0) return false;
  if (match[2] === undefined) return true;
  const month = Number(match[2]);
  if (month < 1 || month > 12) return false;
  if (match[3] === undefined) return true;
  const leap = year % 4 === 0 && (year % 100 !== 0 || year % 400 === 0);
  const days = [31, leap ? 29 : 28, 31, 30, 31, 30, 31, 31, 30, 31, 30, 31];
  return Number(match[3]) >= 1 && Number(match[3]) <= days[month - 1];
}

export function trackReleaseDate(track: Pick<Track, "release_date" | "year">): string {
  return track.release_date ?? (track.year == null ? "" : String(track.year).padStart(4, "0"));
}
