import type { CleanupImportedEvidence, CleanupRuleId } from "@/core/api";

export interface RuleMeta {
  id: CleanupRuleId;
  label: string;
  hint: string;
  defaultOn: boolean;
}

export const RULE_GROUPS: { label: string; rules: RuleMeta[] }[] = [
  {
    label: "Filename fixes",
    rules: [
      {
        id: "strip_track_numbers",
        label: "Strip leading track numbers",
        hint: "“01 - Title” → “Title” (the number can still go to the tag below)",
        defaultOn: true,
      },
      {
        id: "strip_artist",
        label: "Strip artist from filename",
        hint: "“Artist - Title” → “Title” when the artist is tagged, matches the folder, or is shared by the whole folder",
        defaultOn: true,
      },
      {
        id: "strip_album",
        label: "Strip album from filename",
        hint: "Drops a segment matching the album tag or a folder-wide album prefix",
        defaultOn: true,
      },
      {
        id: "strip_junk",
        label: "Remove junk phrases",
        hint: "“(Official Audio)”, “[320kbps]”, site names, “- YouTube”…",
        defaultOn: true,
      },
      {
        id: "normalize_separators",
        label: "Fix separators",
        hint: "Underscores → spaces, %20, doubled spaces",
        defaultOn: true,
      },
      {
        id: "normalize_case",
        label: "Fix ALL-CAPS / all-lowercase names",
        hint: "“MY SONG” → “My Song” — opinionated, so off by default",
        defaultOn: false,
      },
    ],
  },
  {
    label: "Tag fixes (derived from filename + folder)",
    rules: [
      {
        id: "tag_title",
        label: "Set / clean the title tag",
        hint: "Cleans the same residue out of the title tag, or fills it from the cleaned filename",
        defaultOn: true,
      },
      {
        id: "tag_artist",
        label: "Fill empty artist tag",
        hint: "From an “Artist - …” filename prefix",
        defaultOn: true,
      },
      {
        id: "tag_album",
        label: "Fill empty album tag",
        hint: "From an album segment in the filename",
        defaultOn: true,
      },
      {
        id: "tag_number",
        label: "Fill empty track / disc number",
        hint: "From the stripped leading number or a CD1/CD2 subfolder — pre-ticked only when the folder forms a numbered sequence (01, 02, …)",
        defaultOn: true,
      },
      {
        id: "tag_year",
        label: "Fill empty year tag",
        hint: "From a year marker in the folder name — “Album (2013)”, “2019 - Album”",
        defaultOn: true,
      },
    ],
  },
  {
    label: "Folder names",
    rules: [
      {
        id: "rename_folders",
        label: "Rename messy folders",
        hint: "Tidy folder names (underscores, junk), canonicalize disc/part folders to “Disc 1” / “Part 1”, and — when a name is unusable (“1”, artist + junk) — rebuild it from the tracks’ tags (a guess, starts unticked). Case follows the rule above.",
        defaultOn: true,
      },
    ],
  },
];

export const DEFAULT_RULES: CleanupRuleId[] = RULE_GROUPS.flatMap((g) =>
  g.rules.filter((r) => r.defaultOn).map((r) => r.id),
);

export type ScopeType = "all" | "folder" | "tracks";

/** What the next scan covers and which rules and sources it uses. */
export interface CleanupConfig {
  scopeType: ScopeType;
  recursive: boolean;
  rules: Set<CleanupRuleId>;
  useCatalogs: boolean;
  refreshCatalogs: boolean;
  imports: CleanupImportedEvidence[];
}

export function initialCleanupConfig(scopeType: ScopeType): CleanupConfig {
  return {
    scopeType,
    recursive: true,
    rules: new Set(DEFAULT_RULES),
    useCatalogs: true,
    refreshCatalogs: false,
    imports: [],
  };
}

// Mirrors the provider job's bound; its execution-time check remains authoritative.
export const MAX_CATALOG_TRACKS = 500;
