import type { Track } from "@/core/types";

export function TrackMetadataDetails({ track }: { track: Track }) {
  const details = [
    track.composer ? `Composer: ${track.composer}` : "",
    track.release_date ? `Released: ${track.release_date}` : "",
    track.original_release_date ? `Original: ${track.original_release_date}` : "",
  ].filter(Boolean);
  return details.length ? <span className="track-metadata-details muted small">{details.join(" · ")}</span> : null;
}
