import { trackReleaseDate } from "@/core/metadataDate";
import type { Track } from "@/core/types";

export function TrackMetadataDetails({ track }: { track: Track }) {
  const details = [
    track.composer ? `Composer: ${track.composer}` : "",
    trackReleaseDate(track) ? `Released: ${trackReleaseDate(track)}` : "",
    track.original_release_date ? `Original: ${track.original_release_date}` : "",
  ].filter(Boolean);
  return details.length ? <span className="track-metadata-details muted small">{details.join(" · ")}</span> : null;
}
