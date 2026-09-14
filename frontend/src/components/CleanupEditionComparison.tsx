import { useState } from "react";
import type { CleanupEnrichmentPlan, CleanupModelResult, CleanupReleaseChoice } from "@/core/api";
import { CleanupModelReview } from "./CleanupModelReview";

function name(choice: CleanupReleaseChoice) {
  return [choice.title, choice.edition_review?.description, choice.date ?? "Date unknown",
    choice.country === "XW" ? "Worldwide" : choice.country, choice.id.slice(0, 8)].filter(Boolean).join(" · ");
}

export function CleanupEditionComparison({ plans, edition, catalogJobId, disabled, onEdition }: {
  plans: CleanupEnrichmentPlan[]; edition: string | undefined; catalogJobId: string | undefined; disabled: boolean; onEdition: (id: string) => void;
}) {
  const [ai, setAi] = useState<CleanupModelResult | null>(null);
  const choices = [...new Map(plans.flatMap((plan) => plan.release_choices ?? []).map((choice) => [choice.id, choice])).values()];
  const anchor = plans.reduce<CleanupEnrichmentPlan | undefined>((best, plan) =>
    (plan.release_choices?.length ?? 0) > (best?.release_choices?.length ?? 0) ? plan : best, undefined);
  if (!choices.length) return null;
  const automatic = new Set(plans.map((plan) => plan.identity?.release_mbid).filter((id) => id != null));
  const value = edition ?? (automatic.size === 1 ? [...automatic][0] : "");
  const includesAllEditions = (choice: CleanupReleaseChoice) => choices.every((other) => choice.edition_review?.compared_release_ids.includes(other.id));
  const suggested = choices.find((choice) => choice.id === ai?.recommended_release_id);
  return <div className="cleanup-evidence cleanup-edition-comparison">
    <label>Album edition for this folder
      <select value={value} disabled={disabled} onChange={(event) => onEdition(event.target.value)}>
        <option value="">Keep edition unresolved</option>
        {choices.map((choice) => <option key={choice.id} value={choice.id}>{name(choice)}</option>)}
      </select>
    </label>
    <p className="muted small">Choose the release your files came from. This replaces edition suggestions for applicable tracks in this folder; new suggestions start unchecked. Artist and composer suggestions can be reviewed separately.</p>
    <details>
      <summary>Compare editions and tracklist differences</summary>
      <p>Check the original download, booklet or store listing. A file converted to MP3 can still come from a WAV release. If the folder mixes editions, or the evidence is unclear, keep it unresolved.</p>
      <div className="cleanup-edition-cards">
        {choices.map((choice) => {
          const review = choice.edition_review;
          return <article key={choice.id}>
            <h4><a href={`https://musicbrainz.org/release/${encodeURIComponent(choice.id)}`} target="_blank" rel="noreferrer">{name(choice)}</a></h4>
            <p>{choice.artist}{review?.formats.length ? ` · ${review.formats.join(", ")}` : ""}{review?.labels.length ? ` · ${review.labels.join(", ")}` : ""}</p>
            {(choice.barcode || choice.catalog_numbers.length > 0) && <p>Barcode: {choice.barcode || "unknown"} · Catalog number: {choice.catalog_numbers.join(", ") || "unknown"}</p>}
            {review ? <>
              <p>{review.title_matches}/{review.compared_tracks} folder titles match; {review.duration_matches} also agree in duration. Release contains {review.release_tracks} tracks.</p>
              {review.duration_conflicts > 0 && <p>{review.duration_conflicts} matching titles have a duration difference over 10 seconds.</p>}
              {!review.complete && <p>Comparison is incomplete or capped; it cannot establish a preferred edition. Compared {review.compared_tracks} of {review.folder_tracks} folder tracks.</p>}
              {review.recommended && includesAllEditions(choice) && <p><strong>Best supported by the retrieved tracklists.</strong> Every folder title and duration agrees, including a track absent from the other alternatives. Confirm the source before choosing.</p>}
              {review.distinguishing_tracks.length > 0 ? <>
                <p>Tracks absent from the other compared editions:</p>
                <ul>{review.distinguishing_tracks.map((track, i) => <li key={i}>
                  Disc {track.disc_no ?? "?"}, track {track.track_no ?? "?"}: <strong>{track.title}</strong> — {track.present ? track.duration_agrees ? "in your folder; duration agrees" : "title in your folder; duration not confirmed" : "title not found in your folder"}
                </li>)}</ul>
                {review.distinguishing_tracks_total > review.distinguishing_tracks.length && <p>Showing {review.distinguishing_tracks.length} of {review.distinguishing_tracks_total} differences. Open the release for its full tracklist.</p>}
              </> : <p>No unique track titles distinguish this edition. Compare its source, date, format and catalog details.</p>}
              {review.missing_titles_total > 0 && <p>Release titles not found in this folder ({review.missing_titles_total}): {review.missing_titles.join("; ")}{review.missing_titles_total > review.missing_titles.length ? " …" : ""}</p>}
            </> : <p>This saved result predates edition comparisons. Run a fresh catalog lookup to retrieve descriptions and tracklist differences.</p>}
            <p className="muted small">Strict assignment from one supporting track lookup: {choice.assignment.matched}/{choice.assignment.considered}. It also checks existing artist tags. Title/duration comparison above is an edition clue, not recording verification.</p>
          </article>;
        })}
      </div>
      {catalogJobId && anchor && (anchor.release_choices?.length ?? 0) > 1 && anchor.release_choices?.every((choice) => choice.edition_review && includesAllEditions(choice)) && <CleanupModelReview
        key={catalogJobId} trackId={anchor.track_id} catalogJobId={catalogJobId} editionReview onResult={setAi}
      />}
      {ai && <div role="status"><p><strong>AI edition advice:</strong> {ai.decision.reason}</p>
        {suggested && <button type="button" disabled={disabled} onClick={() => onEdition(suggested.id)}>Use suggested edition: {suggested.edition_review?.description || suggested.id.slice(0, 8)}</button>}
      </div>}
    </details>
  </div>;
}
