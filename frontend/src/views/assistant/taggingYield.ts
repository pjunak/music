interface TaggingYield {
  processed_tracks: number;
  tracks_with_suggestions: number;
  tracks_without_suggestions: number;
  suggested_tags: number;
  updated_profiles: number;
}

export function taggingYield(value: unknown): TaggingYield | null {
  if (!value || typeof value !== "object") return null;
  const result = value as Record<string, unknown>;
  const count = (key: string) => typeof result[key] === "number" && Number.isSafeInteger(result[key]) && result[key] >= 0;
  if (!["processed_tracks", "tracks_with_suggestions", "tracks_without_suggestions", "suggested_tags", "updated_profiles"].every(count)) return null;
  const yieldResult = result as unknown as TaggingYield;
  if (yieldResult.processed_tracks !== yieldResult.tracks_with_suggestions + yieldResult.tracks_without_suggestions || yieldResult.updated_profiles > yieldResult.processed_tracks) return null;
  return yieldResult;
}
