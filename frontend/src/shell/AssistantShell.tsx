import type { ComponentType, SVGProps } from "react";
import { NavLink, Outlet } from "react-router-dom";

import {
  LightningIcon,
  MusicNoteIcon,
  SettingsIcon,
  SparkleIcon,
  TagIcon,
} from "@/components/icons";

import { SectionNav } from "./SectionNav";

type AssistantTabIcon = ComponentType<SVGProps<SVGSVGElement>>;

interface AssistantTab {
  to: string;
  label: string;
  shortLabel: string;
  description: string;
  icon: AssistantTabIcon;
}

const ASSISTANT_TABS: AssistantTab[] = [
  {
    to: "playlists",
    label: "Playlist builder",
    shortLabel: "Playlists",
    description: "Plan from a mood or scene",
    icon: MusicNoteIcon,
  },
  {
    to: "eq",
    label: "EQ drafts",
    shortLabel: "EQ",
    description: "Shape a bounded preset",
    icon: LightningIcon,
  },
  {
    to: "moods",
    label: "Mood library",
    shortLabel: "Moods",
    description: "Analyze, suggest, and review",
    icon: TagIcon,
  },
  {
    to: "cleanup",
    label: "Library cleanup",
    shortLabel: "Cleanup",
    description: "Repair files and metadata",
    icon: SparkleIcon,
  },
  {
    to: "ai",
    label: "AI setup",
    shortLabel: "AI",
    description: "Providers and task models",
    icon: SettingsIcon,
  },
];

interface WorkspaceItem {
  to: string;
  label: string;
}

function AssistantWorkspaceShell({
  ariaLabel,
  items,
}: {
  ariaLabel: string;
  items: WorkspaceItem[];
}) {
  return <SectionNav ariaLabel={ariaLabel} items={items} />;
}

/**
 * A separate preparation workspace for local automation and optional
 * model-backed tools. Keeping this outside Authoring prevents generated drafts
 * from complicating the direct editors and their stable write paths.
 */
export function AssistantShell() {
  return (
    <div className="assistant-shell">
      <nav className="assistant-task-nav" aria-label="Assistant sections">
        <div className="assistant-task-nav-intro" aria-hidden="true">
          <span>Assistant workbench</span>
          <strong>Prepare, inspect, then create</strong>
        </div>
        <div className="assistant-task-nav-list">
          {ASSISTANT_TABS.map((item) => {
            const Icon = item.icon;
            return (
              <NavLink
                key={item.to}
                to={item.to}
                aria-label={item.label}
                className={({ isActive }) =>
                  `assistant-task-nav-item${isActive ? " is-active" : ""}`
                }
              >
                <Icon className="assistant-task-nav-icon" />
                <span className="assistant-task-nav-copy">
                  <strong>
                    <span className="assistant-task-nav-label-long">
                      {item.label}
                    </span>
                    <span className="assistant-task-nav-label-short">
                      {item.shortLabel}
                    </span>
                  </strong>
                  <span>{item.description}</span>
                </span>
              </NavLink>
            );
          })}
        </div>
      </nav>
      <div className="assistant-task-body">
        <Outlet />
      </div>
    </div>
  );
}

export function MoodLibraryShell() {
  return (
    <AssistantWorkspaceShell
      ariaLabel="Mood Library sections"
      items={[
        {
          to: "workflow",
          label: "Analysis",
        },
        {
          to: "context",
          label: "Track context",
        },
        {
          to: "tags",
          label: "Mood tags",
        },
        {
          to: "vocabulary",
          label: "Mood vocabulary",
        },
      ]}
    />
  );
}

export function LibraryCleanupShell() {
  return (
    <AssistantWorkspaceShell
      ariaLabel="Library cleanup sections"
      items={[
        {
          to: "run",
          label: "Clean up",
        },
        {
          to: "sources",
          label: "Sources",
        },
        {
          to: "history",
          label: "History & rollback",
        },
      ]}
    />
  );
}
