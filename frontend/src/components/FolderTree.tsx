import { useEffect, useMemo, useRef, useState } from "react";

import {
  ancestorsOf,
  buildFolderIndex,
  filterFolders,
  foldedMatch,
  foldName,
  parentPath,
} from "@/core/folderTreeModel";
import type { FolderNode } from "@/core/folderTreeModel";

import {
  ChevronDownIcon,
  ChevronRightIcon,
  FolderClosedIcon,
  FolderOpenIcon,
  SearchIcon,
  XIcon,
} from "./icons";

/** Folder tree, root-agnostic. The host supplies `loadAll` returning EVERY
 *  folder (any depth, as a flat list — GET /api/library/folders or
 *  /api/sfx/folders) and the tree builds the hierarchy client-side. Having
 *  the whole tree up front is what powers the filter box, type-ahead and
 *  auto-reveal without per-level round trips.
 *
 *  Interaction model:
 *  - Click a row → selects AND expands it (no "two clicks to dive in" tax).
 *    The chevron toggles expansion without changing selection — the escape
 *    hatch for collapsing a subtree without leaving the current folder.
 *  - Filter box: diacritic-insensitive name filter ("dvor" finds "Dvořák");
 *    shows matches plus their ancestors (force-expanded) and highlights the
 *    matched substring. Esc clears.
 *  - Keyboard (WAI-ARIA tree pattern): ↑/↓ move, → expand / into first
 *    child, ← collapse / to parent, Enter or Space select, Home/End, and
 *    type-ahead (letters jump to the next matching folder name). Handled
 *    keys stop propagating so the global shortcuts and SFX hotkeys never
 *    fire off tree navigation.
 *  - When `selectedPath` changes from outside (breadcrumb, post-upload
 *    navigation, reveal-from-search) the tree expands its ancestors and
 *    scrolls the row into view.
 *  - No root row: the implicit root is "everywhere in this library"; the
 *    host's breadcrumb is responsible for navigating back to it.
 *
 *  Drag-and-drop: folder rows are drop targets when `onDropOnFolder` is
 *  supplied. Payloads are JSON; the host decides what shape they carry
 *  (e.g. `{kind: "track", id: 42}`). */

export type TreeFolder = FolderNode;

interface Props {
  /** Selected folder path; "" is root. */
  selectedPath: string;
  /** Called when a folder is clicked / chosen with the keyboard. */
  onSelect: (path: string) => void;
  /** Async fetcher returning every folder in this library, flat. */
  loadAll: () => Promise<TreeFolder[]>;
  /** Trigger value: when this changes the folder list re-fetches
   *  (expansion state is kept). */
  refreshKey?: number | string;
  /** Optional drop handler — when set, folder rows accept drops. */
  onDropOnFolder?: (folderPath: string, payload: unknown) => void;
}

interface VisibleRow {
  node: TreeFolder;
  depth: number;
}

export function FolderTree({
  selectedPath,
  onSelect,
  loadAll,
  refreshKey,
  onDropOnFolder,
}: Props) {
  const [folders, setFolders] = useState<TreeFolder[] | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [expanded, setExpanded] = useState<Set<string>>(new Set());
  const [filter, setFilter] = useState("");
  const [focusedPath, setFocusedPath] = useState<string | null>(null);
  const rowRefs = useRef(new Map<string, HTMLDivElement>());
  const typeahead = useRef({ buf: "", at: 0 });

  useEffect(() => {
    let cancelled = false;
    loadAll()
      .then((all) => {
        if (cancelled) return;
        setError(null);
        setFolders(all);
      })
      .catch((e: unknown) => {
        if (cancelled) return;
        setError(e instanceof Error ? e.message : "load failed");
        setFolders([]);
      });
    return () => {
      cancelled = true;
    };
  }, [loadAll, refreshKey]);

  const index = useMemo(() => buildFolderIndex(folders ?? []), [folders]);

  const query = filter.trim();
  const filterRes = useMemo(
    () => (query === "" ? null : filterFolders(folders ?? [], query)),
    [folders, query],
  );

  // Flat DFS of what's on screen — the keyboard cursor moves through this.
  const visibleRows = useMemo(() => {
    const out: VisibleRow[] = [];
    const walk = (parent: string, depth: number) => {
      for (const node of index.childrenOf.get(parent) ?? []) {
        if (filterRes !== null && !filterRes.visible.has(node.path)) continue;
        out.push({ node, depth });
        if (filterRes !== null || expanded.has(node.path)) walk(node.path, depth + 1);
      }
    };
    walk("", 0);
    return out;
  }, [index, expanded, filterRes]);

  // Auto-reveal: whenever the selection points at a folder we know about,
  // open its ancestors and scroll the row into view — breadcrumb jumps,
  // post-upload navigation and reveal-from-search all land here.
  useEffect(() => {
    if (selectedPath === "" || !index.byPath.has(selectedPath)) return;
    setFocusedPath(selectedPath);
    setExpanded((prev) => {
      const missing = ancestorsOf(selectedPath).filter((a) => !prev.has(a));
      if (missing.length === 0) return prev;
      const next = new Set(prev);
      for (const a of missing) next.add(a);
      return next;
    });
    // Scroll on the next frame, after the expansion above has rendered.
    const raf = window.requestAnimationFrame(() => {
      rowRefs.current.get(selectedPath)?.scrollIntoView({ block: "nearest" });
    });
    return () => window.cancelAnimationFrame(raf);
  }, [selectedPath, index]);

  function toggle(path: string) {
    setExpanded((prev) => {
      const next = new Set(prev);
      if (next.has(path)) next.delete(path);
      else next.add(path);
      return next;
    });
  }

  function selectRow(node: TreeFolder) {
    onSelect(node.path);
    setFocusedPath(node.path);
    // Single click = dive in: selecting also expands. Collapsing is the
    // chevron's job, so re-clicking a selected folder doesn't flap.
    if ((index.childrenOf.get(node.path)?.length ?? 0) > 0) {
      setExpanded((prev) =>
        prev.has(node.path) ? prev : new Set(prev).add(node.path),
      );
    }
  }

  function focusRow(path: string) {
    setFocusedPath(path);
    const el = rowRefs.current.get(path);
    if (el) {
      el.focus();
      el.scrollIntoView({ block: "nearest" });
    }
  }

  function onTreeKeyDown(e: React.KeyboardEvent<HTMLDivElement>) {
    if (visibleRows.length === 0) return;
    if (e.metaKey || e.ctrlKey || e.altKey) return;

    // The key lands on the actually-focused row — trust that over the
    // `focusedPath` state, which can trail behind real DOM focus.
    const targetPath =
      (e.target as HTMLElement).closest?.("[data-tree-path]")?.getAttribute("data-tree-path") ??
      focusedPath;
    const idx = visibleRows.findIndex((r) => r.node.path === targetPath);
    const current = idx >= 0 ? visibleRows[idx] : null;
    const consume = () => {
      e.preventDefault();
      // The window-level shortcut/hotkey listeners must never see keys the
      // tree handled (←/→ would skip tracks, letters would fire SFX).
      e.stopPropagation();
    };
    const moveTo = (i: number) => {
      const clamped = Math.max(0, Math.min(visibleRows.length - 1, i));
      focusRow(visibleRows[clamped].node.path);
    };

    switch (e.key) {
      case "ArrowDown":
        consume();
        moveTo(idx + 1);
        return;
      case "ArrowUp":
        consume();
        moveTo(idx <= 0 ? 0 : idx - 1);
        return;
      case "ArrowRight": {
        consume();
        if (current === null) {
          moveTo(0);
          return;
        }
        const hasKids = (index.childrenOf.get(current.node.path)?.length ?? 0) > 0;
        if (!hasKids) return;
        if (filterRes === null && !expanded.has(current.node.path)) {
          toggle(current.node.path);
        } else {
          moveTo(idx + 1); // already open — its first child is the next row
        }
        return;
      }
      case "ArrowLeft": {
        consume();
        if (current === null) {
          moveTo(0);
          return;
        }
        if (filterRes === null && expanded.has(current.node.path)) {
          toggle(current.node.path);
          return;
        }
        const parent = parentPath(current.node.path);
        if (parent !== "" && index.byPath.has(parent)) focusRow(parent);
        return;
      }
      case "Home":
        consume();
        moveTo(0);
        return;
      case "End":
        consume();
        moveTo(visibleRows.length - 1);
        return;
      case "Enter":
      case " ":
        consume();
        if (current !== null) selectRow(current.node);
        return;
      default: {
        // Type-ahead: printable keys jump to the next folder whose name
        // starts with the accumulated buffer (Explorer behaviour). "/"
        // stays global — it focuses the library search.
        if (e.key.length !== 1 || e.key === "/") return;
        consume();
        const now = Date.now();
        const ta = typeahead.current;
        ta.buf = now - ta.at < 600 ? ta.buf + e.key : e.key;
        ta.at = now;
        const needle = foldName(ta.buf);
        // A repeated single letter cycles through matches; a growing
        // buffer keeps refining from the current row.
        const startAt = idx < 0 ? 0 : ta.buf.length === 1 ? idx + 1 : idx;
        for (let k = 0; k < visibleRows.length; k += 1) {
          const row = visibleRows[(startAt + k) % visibleRows.length];
          if (foldName(row.node.name).startsWith(needle)) {
            focusRow(row.node.path);
            return;
          }
        }
      }
    }
  }

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

  // Exactly one row is tab-reachable (roving tabindex): the focused one,
  // or the first row before the tree has been focused at all.
  const tabPath = visibleRows.some((r) => r.node.path === focusedPath)
    ? focusedPath
    : (visibleRows[0]?.node.path ?? null);

  return (
    <div className="folder-tree">
      <div className="tree-filter">
        <span className="tree-filter-icon" aria-hidden="true">
          <SearchIcon />
        </span>
        <input
          type="search"
          value={filter}
          placeholder="Filter folders"
          aria-label="Filter folders"
          onChange={(e) => setFilter(e.target.value)}
          onKeyDown={(e) => {
            if (e.key === "Escape" && filter !== "") {
              // Clear the filter — not (in the picker) the whole modal.
              e.preventDefault();
              e.stopPropagation();
              setFilter("");
            } else if (e.key === "ArrowDown" && visibleRows.length > 0) {
              e.preventDefault();
              e.stopPropagation();
              focusRow(visibleRows[0].node.path);
            }
          }}
        />
        {filter !== "" ? (
          <button
            type="button"
            className="tree-filter-clear"
            onClick={() => setFilter("")}
            aria-label="Clear filter"
          >
            <XIcon />
          </button>
        ) : null}
      </div>
      {error !== null ? <p className="error small tree-root-error">{error}</p> : null}
      <div className="tree-scroll" onKeyDown={onTreeKeyDown}>
        <div role="tree" aria-label="Folders">
          {visibleRows.map(({ node, depth }) => {
            const hasKids = (index.childrenOf.get(node.path)?.length ?? 0) > 0;
            const isOpen =
              hasKids && (filterRes !== null || expanded.has(node.path));
            const isSelected = selectedPath === node.path;
            const match = filterRes !== null ? foldedMatch(node.name, query) : null;
            return (
              <div
                key={node.path}
                ref={(el) => {
                  if (el) rowRefs.current.set(node.path, el);
                  else rowRefs.current.delete(node.path);
                }}
                role="treeitem"
                data-tree-path={node.path}
                aria-level={depth + 1}
                aria-selected={isSelected}
                {...(hasKids ? { "aria-expanded": isOpen } : {})}
                tabIndex={node.path === tabPath ? 0 : -1}
                className={`tree-row${isSelected ? " selected" : ""}`}
                style={{ "--depth": depth } as React.CSSProperties}
                title={node.path}
                onClick={() => selectRow(node)}
                onFocus={() => setFocusedPath(node.path)}
                {...dropProps(node.path)}
              >
                {hasKids && filterRes === null ? (
                  <span
                    className="tree-toggle"
                    aria-hidden="true"
                    onClick={(e) => {
                      e.stopPropagation();
                      toggle(node.path);
                    }}
                  >
                    {isOpen ? <ChevronDownIcon /> : <ChevronRightIcon />}
                  </span>
                ) : (
                  // Spacer keeps names aligned across sibling rows; leaves
                  // (and filter mode, which force-expands) get no toggle.
                  <span className="tree-toggle tree-toggle-spacer" aria-hidden="true" />
                )}
                <span className="tree-icon">
                  {isOpen ? <FolderOpenIcon /> : <FolderClosedIcon />}
                </span>
                <span className="tree-name">
                  {match !== null ? (
                    <>
                      {node.name.slice(0, match[0])}
                      <mark className="tree-match">
                        {node.name.slice(match[0], match[1])}
                      </mark>
                      {node.name.slice(match[1])}
                    </>
                  ) : (
                    node.name
                  )}
                </span>
                {node.badge !== undefined && node.badge !== null ? (
                  <span className="tree-badge muted small">{node.badge}</span>
                ) : null}
              </div>
            );
          })}
        </div>
        {folders === null && error === null ? (
          <p className="muted small tree-note">Loading…</p>
        ) : filterRes !== null ? (
          <p className="muted small tree-note">
            {filterRes.matches.size} of {folders?.length ?? 0} folders match
          </p>
        ) : folders !== null && folders.length === 0 && error === null ? (
          <p className="muted small tree-note">No folders yet.</p>
        ) : null}
      </div>
    </div>
  );
}
