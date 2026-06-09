import { useCallback, useEffect, useMemo, useRef, useState } from "react";

import {
  ChevronDownIcon,
  ChevronRightIcon,
  FolderClosedIcon,
  FolderOpenIcon,
} from "./icons";

/** Generic folder-tree component, root-agnostic.
 *
 *  The host (LibraryView) supplies a `loadChildren(path)` function that
 *  returns the immediate subfolders + a count blob. This way the tree
 *  works against either the music index or the SFX filesystem with the
 *  same UI.
 *
 *  Interaction model:
 *  - Click a folder row → selects AND expands it (saves the "two clicks
 *    to dive in" tax the original chevron+label split used to demand).
 *  - Click the chevron explicitly → toggle expanded without changing
 *    selection. Used to collapse a subtree without leaving the current
 *    folder.
 *  - No root row: the implicit root is "everywhere in this library". The
 *    host's path breadcrumb is responsible for navigating back to it.
 *
 *  Drag-and-drop:
 *  - Folders are drop targets when `onDropOnFolder` is supplied.
 *  - Drop payloads are JSON; the host decides what shape they carry
 *    (e.g. `{kind: "track", id: 42}`).
 */

export interface TreeFolder {
  name: string;
  path: string;
  /** Whatever count makes sense to show next to the folder name. */
  badge?: number | string | null;
  /** False = this folder has no subfolders; the row renders without an
   *  expand chevron so leaves don't pretend to be expandable. Defaults to
   *  true when omitted (a host that doesn't know yet errs on showing the
   *  toggle — collapsing into "(empty)" still works). */
  hasChildren?: boolean;
}

interface NodeState {
  expanded: boolean;
  loaded: boolean;
  children: TreeFolder[];
  error: string | null;
}

interface Props {
  /** Selected folder path; "" is root. */
  selectedPath: string;
  /** Called when a folder is clicked. */
  onSelect: (path: string) => void;
  /** Async fetcher: given a parent path, return its immediate subfolders. */
  loadChildren: (path: string) => Promise<TreeFolder[]>;
  /** Trigger value: when this changes, the tree reloads root + open folders
   *  (preserving which folders are expanded). */
  refreshKey?: number | string;
  /** Optional drop handler — when set, folder rows accept drops. */
  onDropOnFolder?: (folderPath: string, payload: unknown) => void;
}

export function FolderTree({
  selectedPath,
  onSelect,
  loadChildren,
  refreshKey,
  onDropOnFolder,
}: Props) {
  const [state, setState] = useState<Record<string, NodeState>>({});
  const [rootChildren, setRootChildren] = useState<TreeFolder[] | null>(null);
  const [rootError, setRootError] = useState<string | null>(null);

  // Latest `state` without making `refresh` depend on it (which would re-run
  // the refresh effect on every expand/collapse). Lets refresh read which
  // folders are open so it can restore them.
  const stateRef = useRef(state);
  useEffect(() => {
    stateRef.current = state;
  }, [state]);

  const refresh = useCallback(async () => {
    setRootError(null);
    // Snapshot the open subtrees BEFORE reloading so a rescan / add / rename /
    // delete / upload doesn't collapse the tree (the long-standing bug was
    // `setState({})` here). We re-fetch root + each open folder so badges and
    // newly-created subfolders refresh, while keeping everything expanded.
    const prev = stateRef.current;
    const openPaths = Object.keys(prev).filter(
      (p) => prev[p].expanded && prev[p].loaded,
    );
    try {
      setRootChildren(await loadChildren(""));
    } catch (e) {
      setRootError(e instanceof Error ? e.message : "load failed");
      setRootChildren([]);
      return; // leave expansion state untouched on a root-load failure
    }
    const entries = await Promise.all(
      openPaths.map(async (p) => {
        try {
          const children = await loadChildren(p);
          return [p, { expanded: true, loaded: true, children, error: null }] as const;
        } catch {
          // Folder no longer resolves (deleted / renamed) — drop it.
          return null;
        }
      }),
    );
    const next: Record<string, NodeState> = {};
    for (const e of entries) if (e !== null) next[e[0]] = e[1];
    setState(next);
  }, [loadChildren]);

  useEffect(() => {
    void refresh();
  }, [refresh, refreshKey]);

  const ensureLoaded = useCallback(
    async (path: string) => {
      if (state[path]?.loaded) return;
      try {
        const kids = await loadChildren(path);
        setState((prev) => ({
          ...prev,
          [path]: { expanded: true, loaded: true, children: kids, error: null },
        }));
      } catch (e) {
        setState((prev) => ({
          ...prev,
          [path]: {
            expanded: true,
            loaded: true,
            children: [],
            error: e instanceof Error ? e.message : "load failed",
          },
        }));
      }
    },
    [state, loadChildren],
  );

  const toggle = useCallback(
    (path: string) => {
      const cur = state[path];
      if (!cur || !cur.loaded) {
        void ensureLoaded(path);
        return;
      }
      setState((prev) => ({
        ...prev,
        [path]: { ...cur, expanded: !cur.expanded },
      }));
    },
    [state, ensureLoaded],
  );

  function dropProps(folderPath: string) {
    if (!onDropOnFolder) return {};
    return {
      onDragOver: (e: React.DragEvent<HTMLDivElement>) => {
        e.preventDefault();
        e.dataTransfer.dropEffect = "move";
      },
      onDrop: (e: React.DragEvent<HTMLDivElement>) => {
        e.preventDefault();
        const raw = e.dataTransfer.getData("application/json");
        if (!raw) return;
        try {
          const payload = JSON.parse(raw) as unknown;
          onDropOnFolder(folderPath, payload);
        } catch {
          /* ignore malformed drop */
        }
      },
    };
  }

  const renderRow = (folder: TreeFolder, depth: number) => {
    const node = state[folder.path];
    const expanded = node?.expanded ?? false;
    const isSelected = selectedPath === folder.path;
    // A folder is a leaf when the host has explicitly told us so. If the
    // host doesn't know (legacy / undefined), keep the toggle visible so
    // we don't accidentally hide a real subtree.
    const isLeaf = folder.hasChildren === false;
    return (
      <div key={folder.path}>
        <div
          className={`tree-row${isSelected ? " selected" : ""}${isLeaf ? " tree-row-leaf" : ""}`}
          style={{ "--depth": depth } as React.CSSProperties}
          {...dropProps(folder.path)}
        >
          {isLeaf ? (
            // Spacer keeps the folder name aligned with sibling rows that
            // do show a chevron. aria-hidden so screen readers don't see
            // a phantom control.
            <span className="tree-toggle tree-toggle-spacer" aria-hidden="true" />
          ) : (
            <button
              type="button"
              className="tree-toggle btn-ghost"
              // stopPropagation so a chevron click doesn't also trigger
              // the row's select-and-expand handler — chevron is the
              // operator's escape hatch for "I just want to collapse,
              // not change selection".
              onClick={(e) => {
                e.stopPropagation();
                toggle(folder.path);
              }}
              aria-label={expanded ? "Collapse" : "Expand"}
            >
              {expanded ? <ChevronDownIcon /> : <ChevronRightIcon />}
            </button>
          )}
          <button
            type="button"
            className="tree-label btn-ghost"
            onClick={() => {
              onSelect(folder.path);
              // Single click = dive in. If the folder has children and
              // isn't already expanded, also expand it so the operator
              // sees what's underneath without a second click. Leaves
              // skip this branch (nothing to expand).
              if (!isLeaf && !expanded) {
                void ensureLoaded(folder.path);
              }
            }}
            title={folder.path}
          >
            <span className="tree-icon">
              {expanded ? <FolderOpenIcon /> : <FolderClosedIcon />}
            </span>
            <span className="tree-name">{folder.name}</span>
            {folder.badge !== undefined && folder.badge !== null ? (
              <span className="tree-badge muted small">{folder.badge}</span>
            ) : null}
          </button>
        </div>
        {expanded && node?.loaded && !isLeaf ? (
          <div className="tree-children">
            {node.error !== null ? (
              <p
                className="error small tree-note"
                style={{ "--depth": depth + 1 } as React.CSSProperties}
              >
                {node.error}
              </p>
            ) : node.children.length === 0 ? (
              <p
                className="muted small tree-note"
                style={{ "--depth": depth + 1 } as React.CSSProperties}
              >
                (empty)
              </p>
            ) : (
              node.children.map((c) => renderRow(c, depth + 1))
            )}
          </div>
        ) : null}
      </div>
    );
  };

  const rootChildrenView = useMemo(() => rootChildren ?? [], [rootChildren]);

  return (
    <div className="folder-tree">
      {/* No explicit root row: the old "All music" / "All SFX" entry was
          redundant (the tab is already "Library") and selecting nothing at
          all IS the root state — the host's breadcrumb path makes that
          explicit. Move-to-root via drag isn't offered here; the Library's
          Move… affordance (FolderActions / SelectionToolbar, with a root
          option) covers that rare case. */}
      {rootError !== null ? (
        <p className="error small tree-root-error">{rootError}</p>
      ) : null}
      <div className="tree-children">
        {rootChildrenView.map((c) => renderRow(c, 0))}
      </div>
    </div>
  );
}
