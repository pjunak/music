import { SectionNav } from "./SectionNav";

const ASSISTANT_TABS = [
  { to: "playlists", label: "Playlist Builder" },
  { to: "analysis", label: "Library Analysis" },
  { to: "cleanup", label: "Cleanup" },
];

/**
 * A separate preparation workspace for local automation and future optional
 * model-backed tools. Keeping this outside Authoring prevents experiments
 * from complicating the direct editors and their stable write paths.
 */
export function AssistantShell() {
  return <SectionNav ariaLabel="Assistant sections" items={ASSISTANT_TABS} />;
}
