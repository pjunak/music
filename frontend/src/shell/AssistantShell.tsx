import { SectionNav } from "./SectionNav";

const ASSISTANT_TABS = [
  { to: "playlists", label: "Playlist Builder" },
  { to: "analysis", label: "Library Analysis" },
  { to: "ai", label: "AI Setup" },
  { to: "cleanup", label: "Cleanup" },
];

/**
 * A separate preparation workspace for local automation and optional
 * model-backed tools. Keeping this outside Authoring prevents generated drafts
 * from complicating the direct editors and their stable write paths.
 */
export function AssistantShell() {
  return <SectionNav ariaLabel="Assistant sections" items={ASSISTANT_TABS} />;
}
