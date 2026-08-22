import { SectionNav } from "./SectionNav";

const ASSISTANT_TABS = [
  { to: "playlists", label: "Playlist Builder" },
  { to: "eq", label: "EQ Assistant" },
  { to: "analysis", label: "Library Analysis" },
  { to: "tags", label: "Tag Vocabulary" },
  { to: "ai", label: "AI Setup" },
];

/**
 * A separate preparation workspace for local automation and optional
 * model-backed tools. Keeping this outside Authoring prevents generated drafts
 * from complicating the direct editors and their stable write paths.
 */
export function AssistantShell() {
  return <SectionNav ariaLabel="Assistant sections" items={ASSISTANT_TABS} />;
}
