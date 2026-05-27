import { useCallback, useEffect, useMemo, useState } from "react";

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
  /** Trigger value: when this changes, the tree clears its cache and reloads. */
  refreshKey?: number | string;
  /** Optional drop handler — when set, folder rows accept drops. */
  onDropOnFolder?: (folderPath: string, payload: unknown) => void;
  /** Optional "drop here = move to root" payload handler. Renders an
   *  invisible drop strip at the top of the tree so the operator can move
   *  things back to the library root without a visible root row. */
  rootDropLabel?: string;
}

export function FolderTree({
  selectedPath,
  onSelect,
  loadChildren,
  refreshKey,
  onDropOnFolder,
  rootDropLabel,
}: Props) {
  const [state, setState] = useState<Record<string, NodeState>>({});
  const [rootChildren, setRootChildren] = useState<TreeFolder[] | null>(null);
  const [rootError, setRootError] = useState<string | null>(null);

  const refresh = useCallback(async () => {
    setRootError(null);
    try {
      const kids = await loadChildren("");
      setRootChildren(kids);
    } catch (e) {
      setRootError(e instanceof Error ? e.message : "load failed");
      setRootChildren([]);
    }
    setState({}); // collapse all on refresh
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
          style={{ paddingLeft: `${depth * 0.9 + 0.4}rem` }}
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
                className="error small"
                style={{ paddingLeft: `${(depth + 1) * 0.9 + 0.4}rem` }}
              >
                {node.error}
              </p>
            ) : node.children.length === 0 ? (
              <p
                className="muted small"
                style={{ paddingLeft: `${(depth + 1) * 0.9 + 0.4}rem` }}
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
      {/* The root row used to be an explicit "All music" / "All SFX" entry
          and the chevron showed as decoration. The label was redundant
          (the tab itself is "Library"), and reserving a click slot for
          "go to root" was wasted real estate — selecting nothing at all
          IS the root state, and the host's breadcrumb path makes that
          explicit. We keep an invisible drop strip at the top, though, so
          a drag-to-root operation is still possible. */}
      {onDropOnFolder !== undefined ? (
        <div
          className={`tree-row-root-drop${selectedPath === "" ? " selected" : ""}`}
          aria-label={rootDropLabel ?? "Drop to move to root"}
          {...dropProps("")}
        />
      ) : null}
      {rootError !== null ? (
        <p className="error small" style={{ padding: "0.4rem" }}>
          {rootError}
        </p>
      ) : null}
      <div className="tree-children">
        {rootChildrenView.map((c) => renderRow(c, 0))}
      </div>
    </div>
  );
}
