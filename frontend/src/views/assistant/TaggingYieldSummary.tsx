import { taggingYield } from "./taggingYield";

export function TaggingYieldSummary({ value, partial = false }: { value: unknown; partial?: boolean }) {
  const result = taggingYield(value);
  if (!result) return null;
  return <div aria-label={partial ? "Saved partial tagging outcome" : "Tagging yield"}>
    {partial ? <strong>Completed work retained</strong> : null}
    <p>{result.processed_tracks} tracks analysed: {result.tracks_with_suggestions} with suggestions, {result.tracks_without_suggestions} with no supported tags. {result.suggested_tags} tags returned.</p>
    {partial ? <p>{result.updated_profiles} profiles saved before the run stopped. Further tracks may not have completed; check recorded provider usage before retrying uncertain work.</p> : null}
  </div>;
}
