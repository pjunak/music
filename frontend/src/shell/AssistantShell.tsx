import { NavLink, Outlet } from "react-router-dom";

import { SectionNav } from "./SectionNav";

const ASSISTANT_TABS = [
  { to: "playlists", label: "Playlist Builder" },
  { to: "eq", label: "EQ Assistant" },
  { to: "moods", label: "Mood Library" },
  { to: "settings", label: "Settings" },
];

interface WorkspaceItem {
  to: string;
  label: string;
  description: string;
}

function AssistantWorkspaceShell({
  ariaLabel,
  items,
}: {
  ariaLabel: string;
  items: WorkspaceItem[];
}) {
  return (
    <div className="assistant-workspace-section">
      <nav className="assistant-workspace-nav" aria-label={ariaLabel}>
        {items.map((item) => (
          <NavLink
            key={item.to}
            to={item.to}
            className={({ isActive }) =>
              `assistant-workspace-nav-item${isActive ? " is-active" : ""}`
            }
          >
            <strong>{item.label}</strong>
            <span>{item.description}</span>
          </NavLink>
        ))}
      </nav>
      <div className="assistant-workspace-section-body">
        <Outlet />
      </div>
    </div>
  );
}

/**
 * A separate preparation workspace for local automation and optional
 * model-backed tools. Keeping this outside Authoring prevents generated drafts
 * from complicating the direct editors and their stable write paths.
 */
export function AssistantShell() {
  return <SectionNav ariaLabel="Assistant sections" items={ASSISTANT_TABS} />;
}

export function MoodLibraryShell() {
  return (
    <AssistantWorkspaceShell
      ariaLabel="Mood Library sections"
      items={[
        {
          to: "workflow",
          label: "Analyze and tag",
          description: "Build evidence, suggest moods, and review changes",
        },
        {
          to: "context",
          label: "Track context",
          description: "Inspect the factual evidence behind suggestions",
        },
      ]}
    />
  );
}

export function AssistantSettingsShell() {
  return (
    <AssistantWorkspaceShell
      ariaLabel="Assistant settings sections"
      items={[
        {
          to: "models",
          label: "Models and providers",
          description: "Connections, routing, tests, and limits",
        },
        {
          to: "vocabulary",
          label: "Mood vocabulary",
          description: "Terrain, scene, and mood choices",
        },
      ]}
    />
  );
}
