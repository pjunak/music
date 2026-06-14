import { useCallback, useEffect, useMemo, useRef, useState } from "react";
import type { ChangeEvent, Dispatch, DragEvent, MouseEvent, SetStateAction } from "react";

import { Breadcrumb } from "@/components/Breadcrumb";
import type { BreadcrumbItem } from "@/components/Breadcrumb";
import { CleanupDialog } from "@/components/CleanupDialog";
import { confirmDialog } from "@/components/confirmDialog";
import { EmptyState } from "@/components/EmptyState";
import { FolderPickerModal } from "@/components/FolderPickerModal";
import { FolderTree } from "@/components/FolderTree";
import type { TreeFolder } from "@/components/FolderTree";
import { IconButton } from "@/components/IconButton";
import {
  EditIcon,
  FolderOpenIcon,
  ImportIcon,
  LightningIcon,
  MoveIcon,
  MusicNoteIcon,
  PlayIcon,
  PlusIcon,
  RescanIcon,
  SearchIcon,
  ShuffleIcon,
  SparkleIcon,
  TrashIcon,
  XIcon,
} from "@/components/icons";
import { inputDialog } from "@/components/inputDialog";
import { TagInspector } from "@/components/TagInspector";
import { libraryApi, sfxApi } from "@/core/api";
import type { SfxFile } from "@/core/api";
import {
  collectEntries,
  entriesFromItems,
  groupByParent,
  isAudioFile,
} from "@/core/dropTraversal";
import type { CollectedFile } from "@/core/dropTraversal";
import { selectActiveTrackId, usePlayerStore } from "@/core/playerStore";
import { toast } from "@/core/toast";
import { trackTitle } from "@/core/trackDisplay";
import type { Track } from "@/core/types";
import { useDebouncedValue } from "@/core/useDebouncedValue";
import { useUiStore } from "@/core/uiStore";
import { wsClient } from "@/core/ws";

type Root = "music" | "sfx";

function formatSize(bytes: number): string {
  if (bytes < 1024) return `${bytes} B`;
  if (bytes < 1024 * 1024) return `${(bytes / 1024).toFixed(1)} KB`;
  if (bytes < 1024 * 1024 * 1024) return `${(bytes / (1024 * 1024)).toFixed(1)} MB`;
  return `${(bytes / (1024 * 1024 * 1024)).toFixed(2)} GB`;
}

function formatDuration(seconds: number): string {
  if (!seconds || seconds <= 0) return "—";
  const total = Math.floor(seconds);
  return `${Math.floor(total / 60)}:${(total % 60).toString().padStart(2, "0")}`;
}

const musicAllFolders = async (): Promise<TreeFolder[]> =>
  (await libraryApi.allFolders()).folders.map((f) => ({
    name: f.name,
    path: f.path,
    badge: f.track_count > 0 ? f.track_count : null,
  }));

const sfxAllFolders = async (): Promise<TreeFolder[]> =>
  (await sfxApi.allFolders()).folders.map((f) => ({
    name: f.name,
    path: f.path,
    badge: f.file_count > 0 ? f.file_count : null,
  }));

export function LibraryView() {
  const [root, setRoot] = useState<Root>("music");
  const [path, setPath] = useState("");
  const [refreshKey, setRefreshKey] = useState(0);
  const [pendingQuery, setPendingQuery] = useState("");
  const [query, setQuery] = useState("");
  // Ticked-checkbox selection of the music list. Lives here (not in
  // MusicWorkspace) so the toolbar's Clean up dialog can offer
  // "selected tracks" as a scope.
  const [checked, setChecked] = useState<Set<number>>(new Set());
  const [cleanupOpen, setCleanupOpen] = useState(false);

  // The operator-resized tree width overrides the `--rail-tree` token on
  // this view only. Applied as a direct CSS-var write (not a React style
  // prop) so SidebarRail's drag handler can write the same var during the
  // drag without fighting React's style reconciliation.
  const treeWidth = useUiStore((s) => s.libraryTreeWidth);
  const viewRef = useRef<HTMLDivElement | null>(null);
  useEffect(() => {
    const el = viewRef.current;
    if (el === null) return;
    if (treeWidth !== null) el.style.setProperty("--rail-tree", `${treeWidth}px`);
    else el.style.removeProperty("--rail-tree");
  }, [treeWidth]);

  // Search-as-you-type: debounce keystrokes and auto-submit the result.
  // The input is always visible in the toolbar (music mode only), so the
  // user never has to click "search" first. A non-empty `query` switches
  // the body from folder-browser to search results; clearing the input
  // drops back to the folder browser at the previously-selected path.
  const debouncedPending = useDebouncedValue(pendingQuery, 250);
  useEffect(() => {
    if (debouncedPending !== query) setQuery(debouncedPending);
  }, [debouncedPending, query]);

  function clearSearch() {
    setPendingQuery("");
    setQuery("");
  }

  function selectRoot(next: Root) {
    setRoot(next);
    setPath("");
    clearSearch();
    setChecked(new Set());
  }

  return (
    <div className="library-view" ref={viewRef}>
      <header className="library-toolbar">
        <div className="segmented" role="tablist" aria-label="Library root">
          <button
            type="button"
            role="tab"
            aria-selected={root === "music"}
            className="segmented-item"
            onClick={() => selectRoot("music")}
          >
            <MusicNoteIcon aria-hidden="true" /> Music
          </button>
          <button
            type="button"
            role="tab"
            aria-selected={root === "sfx"}
            className="segmented-item"
            onClick={() => selectRoot("sfx")}
          >
            <LightningIcon aria-hidden="true" /> SFX
          </button>
        </div>
        {root === "music" ? (
          <div className="library-toolbar-search">
            <span className="library-toolbar-search-icon" aria-hidden="true">
              <SearchIcon />
            </span>
            <input
              type="search"
              value={pendingQuery}
              onChange={(e) => setPendingQuery(e.target.value)}
              placeholder="Search title / artist / album / path"
              aria-label="Search music library"
            />
            {pendingQuery !== "" ? (
              <button
                type="button"
                className="library-toolbar-search-clear"
                onClick={clearSearch}
                aria-label="Clear search"
                title="Clear"
              >
                <XIcon />
              </button>
            ) : null}
          </div>
        ) : null}
        {root === "music" ? (
          <IconButton
            label="Find and batch-fix common filename/tag issues"
            icon={<SparkleIcon />}
            className="library-cleanup-btn"
            onClick={() => setCleanupOpen(true)}
          >
            Clean up
          </IconButton>
        ) : null}
        <RescanButton root={root} onComplete={() => setRefreshKey((k) => k + 1)} />
      </header>

      {cleanupOpen && root === "music" ? (
        <CleanupDialog
          path={path}
          checkedIds={[...checked]}
          onClose={() => setCleanupOpen(false)}
          onApplied={() => setRefreshKey((k) => k + 1)}
        />
      ) : null}

      {root === "music" ? (
        <MusicWorkspace
          path={path}
          onPathChange={setPath}
          query={query}
          refreshKey={refreshKey}
          onRefresh={() => setRefreshKey((k) => k + 1)}
          checked={checked}
          setChecked={setChecked}
          onRevealFolder={(p) => {
            // Search result → browse mode at that folder; the tree's
            // auto-reveal expands down to it.
            clearSearch();
            setPath(p);
          }}
        />
      ) : (
        <SfxBrowser
          path={path}
          onPathChange={setPath}
          refreshKey={refreshKey}
          onRefresh={() => setRefreshKey((k) => k + 1)}
        />
      )}
    </div>
  );
}

// --- shared bits ---------------------------------------------------------

function RescanButton({
  root,
  onComplete,
}: {
  root: Root;
  onComplete: () => void;
}) {
  const [busy, setBusy] = useState(false);
  if (root === "sfx") {
    // SFX doesn't have an index — refreshing the tree is enough.
    return (
      <IconButton
        label="Refresh the folder view"
        icon={<RescanIcon />}
        className="library-rescan"
        onClick={onComplete}
      >
        Refresh
      </IconButton>
    );
  }
  return (
    <IconButton
      label="Rescan the music library from disk"
      icon={<RescanIcon />}
      className="library-rescan"
      disabled={busy}
      onClick={async () => {
        setBusy(true);
        try {
          const r = await libraryApi.rescan();
          const parts: string[] = [];
          if (r.added) parts.push(`+${r.added} added`);
          if (r.updated) parts.push(`${r.updated} updated`);
          if (r.removed) parts.push(`-${r.removed} removed`);
          toast.success("Rescan complete", parts.join(", ") || "No changes.");
          onComplete();
        } catch (e) {
          toast.error("Rescan failed", e instanceof Error ? e.message : undefined);
        } finally {
          setBusy(false);
        }
      }}
    >
      {busy ? "Rescanning…" : "Rescan"}
    </IconButton>
  );
}

// --- MUSIC ---------------------------------------------------------------

/** The unified music screen: wide folder tree (browse mode) + a multi-select
 *  Name/File track list + a right-hand TagInspector that edits the selection.
 *  Search and browse share the same list + inspector; search just drops the
 *  tree + upload zone (a search is already a global filter). */
function MusicWorkspace({
  path,
  onPathChange,
  query,
  refreshKey,
  onRefresh,
  checked,
  setChecked,
  onRevealFolder,
}: {
  path: string;
  onPathChange: (p: string) => void;
  query: string;
  refreshKey: number;
  onRefresh: () => void;
  /** Ticked-checkbox set, owned by LibraryView (the cleanup dialog reads
   *  it as a scope). Only a direct checkbox click, the header select-all,
   *  Ctrl/Cmd-click, or a Shift range mutates it. */
  checked: Set<number>;
  setChecked: Dispatch<SetStateAction<Set<number>>>;
  /** Leave search mode and browse to this folder (reveal-from-search). */
  onRevealFolder: (path: string) => void;
}) {
  const activeTrackId = usePlayerStore(selectActiveTrackId);
  const [tracks, setTracks] = useState<Track[]>([]);
  const [error, setError] = useState<string | null>(null);
  const [loading, setLoading] = useState(false);
  // `focused` stays local: the single highlighted row a plain click
  // selects, which feeds the tag inspector. A plain click (or
  // double-click to play) must NOT tick a checkbox.
  const [focused, setFocused] = useState<number | null>(null);
  const searching = query !== "";

  useEffect(() => {
    let cancelled = false;
    setLoading(true);
    setError(null);
    const fetcher = searching
      ? libraryApi
          .search({ q: query, limit: 200, sort: "artist", order: "asc" })
          .then((r) => r.tracks)
      : libraryApi.tree(path).then((r) => r.tracks);
    void fetcher
      .then((ts) => {
        if (cancelled) return;
        setTracks(ts);
        // Prune selection state that fell out of the now-visible set.
        const visible = new Set(ts.map((t) => t.id));
        setChecked((prev) => new Set([...prev].filter((id) => visible.has(id))));
        setFocused((prev) => (prev !== null && visible.has(prev) ? prev : null));
      })
      .catch((e: unknown) => {
        if (!cancelled) {
          setError(e instanceof Error ? e.message : "load failed");
          setTracks([]);
        }
      })
      .finally(() => {
        if (!cancelled) setLoading(false);
      });
    return () => {
      cancelled = true;
    };
  }, [searching, query, path, refreshKey, setChecked]);

  const loadAll = useCallback(musicAllFolders, []);

  function revealTrack(t: Track) {
    setFocused(t.id);
    onRevealFolder(t.path.split("/").slice(0, -1).join("/"));
  }

  async function onDropOnFolder(folderPath: string, payload: unknown) {
    if (
      !payload ||
      typeof payload !== "object" ||
      (payload as { kind?: unknown }).kind !== "music-track"
    )
      return;
    const id = (payload as { id?: number }).id;
    if (typeof id !== "number") return;
    try {
      await libraryApi.moveTrack(id, folderPath);
      toast.success("Track moved");
      onRefresh();
    } catch (e) {
      toast.error("Move failed", e instanceof Error ? e.message : undefined);
    }
  }

  // The inspector edits the ticked rows when there's a checkbox selection;
  // otherwise it edits the single plain-click-highlighted row.
  const inspectorTracks = useMemo(() => {
    if (checked.size > 0) return tracks.filter((t) => checked.has(t.id));
    if (focused !== null) {
      const t = tracks.find((t) => t.id === focused);
      return t ? [t] : [];
    }
    return [];
  }, [tracks, checked, focused]);

  return (
    <div className={`music-workspace${searching ? " no-tree" : ""}`}>
      {!searching ? (
        <SidebarRail>
          <FolderTree
            selectedPath={path}
            onSelect={onPathChange}
            loadAll={loadAll}
            refreshKey={refreshKey}
            onDropOnFolder={onDropOnFolder}
          />
          <FolderActions
            root="music"
            selectedPath={path}
            onChanged={onRefresh}
            onPathReset={() => onPathChange("")}
          />
        </SidebarRail>
      ) : null}
      <section className={`library-main${checked.size > 0 ? " has-selection" : ""}`}>
        {searching ? (
          <div className="folder-band folder-band-search">
            <span className="muted small">
              {loading
                ? "Searching…"
                : `${tracks.length} match${tracks.length === 1 ? "" : "es"} for “${query}”`}
            </span>
          </div>
        ) : (
          <FolderBand
            root="music"
            dest={path}
            onPathSelect={onPathChange}
            onUploaded={onRefresh}
            onNavigate={onPathChange}
          />
        )}
        {error !== null ? <p className="error small">{error}</p> : null}
        <MusicTrackList
          tracks={tracks}
          loading={loading}
          activeTrackId={activeTrackId}
          checked={checked}
          onCheckedChange={setChecked}
          focused={focused}
          onFocusedChange={setFocused}
          draggable={!searching}
          onChanged={onRefresh}
          {...(searching ? { onReveal: revealTrack } : {})}
        />
        <SelectionToolbar
          selected={checked}
          total={tracks.length}
          onClear={() => setChecked(new Set())}
          onChanged={() => {
            setChecked(new Set());
            setFocused(null);
            onRefresh();
          }}
        />
      </section>
      <aside className="library-inspector">
        <TagInspector selectedTracks={inspectorTracks} onSaved={onRefresh} />
      </aside>
    </div>
  );
}

/** Shell shared by the Music and SFX browsers — folder tree + folder
 *  actions on the left, header + upload + error + caller-supplied content
 *  on the right. The rest of the browser-specific behaviour (which API to
 *  hit, what to render in the right pane) is the caller's responsibility,
 *  but the layout shape is defined exactly once. */
function LibraryShell({
  rootKind,
  selectedPath,
  onPathChange,
  refreshKey,
  loadAll,
  onDropOnFolder,
  onRefresh,
  onPathReset,
  error,
  children,
}: {
  rootKind: Root;
  selectedPath: string;
  onPathChange: (p: string) => void;
  refreshKey: number;
  loadAll: () => Promise<TreeFolder[]>;
  onDropOnFolder?: (folderPath: string, payload: unknown) => void;
  onRefresh: () => void;
  onPathReset: () => void;
  error: string | null;
  children: React.ReactNode;
}) {
  return (
    <div className="library-body">
      <SidebarRail>
        <FolderTree
          selectedPath={selectedPath}
          onSelect={onPathChange}
          loadAll={loadAll}
          {...(refreshKey !== undefined ? { refreshKey } : {})}
          {...(onDropOnFolder !== undefined ? { onDropOnFolder } : {})}
        />
        <FolderActions
          root={rootKind}
          selectedPath={selectedPath}
          onChanged={onRefresh}
          onPathReset={onPathReset}
        />
      </SidebarRail>
      <section className="library-main">
        <FolderBand
          root={rootKind}
          dest={selectedPath}
          onPathSelect={onPathChange}
          onUploaded={onRefresh}
          onNavigate={onPathChange}
        />
        {error !== null ? <p className="error small">{error}</p> : null}
        {children}
      </section>
    </div>
  );
}

const RAIL_MIN = 240;
const RAIL_MAX = 560;

/** The tree-column <aside> plus its drag-to-resize right edge. The chosen
 *  width is a persisted preference (uiStore.libraryTreeWidth) consumed as
 *  the `--rail-tree` var by the library grids. During a drag the var is
 *  written straight onto the `.library-view` element so layout tracks the
 *  pointer without a store commit (= React render) per move; the store is
 *  committed once on release. Double-click resets to the stylesheet
 *  default. */
function SidebarRail({ children }: { children: React.ReactNode }) {
  const setWidth = useUiStore((s) => s.setLibraryTreeWidth);
  const asideRef = useRef<HTMLElement | null>(null);

  function onPointerDown(e: React.PointerEvent<HTMLDivElement>) {
    const aside = asideRef.current;
    const host = aside?.closest<HTMLElement>(".library-view");
    if (!aside || !host) return;
    e.preventDefault();
    const handle = e.currentTarget;
    handle.setPointerCapture(e.pointerId);
    const startX = e.clientX;
    const startW = aside.getBoundingClientRect().width;
    let lastW = startW;
    const onMove = (ev: PointerEvent) => {
      ev.preventDefault();
      lastW = Math.round(
        Math.max(RAIL_MIN, Math.min(RAIL_MAX, startW + (ev.clientX - startX))),
      );
      host.style.setProperty("--rail-tree", `${lastW}px`);
    };
    const onUp = () => {
      handle.removeEventListener("pointermove", onMove);
      handle.removeEventListener("pointerup", onUp);
      handle.removeEventListener("pointercancel", onUp);
      // Commit; LibraryView's effect re-applies the same var from the store.
      setWidth(lastW);
    };
    handle.addEventListener("pointermove", onMove);
    handle.addEventListener("pointerup", onUp);
    handle.addEventListener("pointercancel", onUp);
  }

  return (
    <aside ref={asideRef} className="library-sidebar">
      {children}
      <div
        className="rail-resize"
        role="separator"
        aria-orientation="vertical"
        aria-label="Resize the folder tree column"
        title="Drag to resize — double-click to reset"
        onPointerDown={onPointerDown}
        onDoubleClick={() => setWidth(null)}
      />
    </aside>
  );
}

// --- SFX -----------------------------------------------------------------

function SfxBrowser({
  path,
  onPathChange,
  refreshKey,
  onRefresh,
}: {
  path: string;
  onPathChange: (p: string) => void;
  refreshKey: number;
  onRefresh: () => void;
}) {
  const [files, setFiles] = useState<SfxFile[]>([]);
  const [error, setError] = useState<string | null>(null);

  useEffect(() => {
    let cancelled = false;
    setError(null);
    void sfxApi
      .tree(path)
      .then((r) => {
        if (!cancelled) setFiles(r.files);
      })
      .catch((e: unknown) => {
        if (!cancelled) {
          setError(e instanceof Error ? e.message : "load failed");
          setFiles([]);
        }
      });
    return () => {
      cancelled = true;
    };
  }, [path, refreshKey]);

  const loadAll = useCallback(sfxAllFolders, []);

  async function onDropOnFolder(folderPath: string, payload: unknown) {
    if (
      !payload ||
      typeof payload !== "object" ||
      (payload as { kind?: unknown }).kind !== "sfx-file"
    )
      return;
    const filePath = (payload as { path?: string }).path;
    if (typeof filePath !== "string") return;
    try {
      await sfxApi.moveFile(filePath, folderPath);
      toast.success("SFX moved");
      onRefresh();
    } catch (e) {
      toast.error("Move failed", e instanceof Error ? e.message : undefined);
    }
  }

  async function onDelete(file: SfxFile) {
    const ok = await confirmDialog({
      title: "Delete SFX file?",
      body: `${file.path} will be removed from disk.`,
      tone: "danger",
      confirmLabel: "Delete",
    });
    if (!ok) return;
    try {
      await sfxApi.deleteFile(file.path);
      toast.success("Deleted", file.path);
      onRefresh();
    } catch (e) {
      toast.error("Delete failed", e instanceof Error ? e.message : undefined);
    }
  }

  function preview(file: SfxFile) {
    if (!file.referenced) {
      toast.warn(
        "Preview unavailable",
        "Server only streams SFX referenced by a soundboard. Add it to a soundboard first.",
      );
      return;
    }
    const url = sfxApi.fileUrl(file.path);
    new Audio(url).play().catch(() => {
      toast.error("Preview failed", "Browser refused to play the file.");
    });
  }

  return (
    <LibraryShell
      rootKind="sfx"
      selectedPath={path}
      onPathChange={onPathChange}
      refreshKey={refreshKey}
      loadAll={loadAll}
      onDropOnFolder={onDropOnFolder}
      onRefresh={onRefresh}
      onPathReset={() => onPathChange("")}
      error={error}
    >
      {files.length === 0 ? (
          <EmptyState title="No SFX files in this folder">
            Drop audio files into the zone above, or pick a different folder
            from the tree on the left.
          </EmptyState>
        ) : (
          <ul className="sfx-file-list">
            {files.map((f) => (
              <li
                key={f.path}
                className="sfx-file-row"
                draggable
                onDragStart={(e) => {
                  e.dataTransfer.setData(
                    "application/json",
                    JSON.stringify({ kind: "sfx-file", path: f.path }),
                  );
                  e.dataTransfer.effectAllowed = "move";
                }}
              >
                <span className="sfx-file-name">{f.name}</span>
                {f.referenced ? (
                  <span className="badge badge-ok">referenced</span>
                ) : (
                  <span className="muted small">unreferenced</span>
                )}
                <span className="muted small">{formatSize(f.size_bytes)}</span>
                <div className="sfx-file-actions">
                  <IconButton
                    label={
                      f.referenced
                        ? "Preview SFX"
                        : "Preview unavailable — add to a soundboard first"
                    }
                    icon={<PlayIcon />}
                    disabled={!f.referenced}
                    onClick={() => preview(f)}
                  />
                  <IconButton
                    label="Delete SFX file"
                    icon={<TrashIcon />}
                    variant="danger"
                    onClick={() => void onDelete(f)}
                  />
                </div>
              </li>
            ))}
          </ul>
        )}
    </LibraryShell>
  );
}

// --- shared sub-components ----------------------------------------------

function FolderHeader({
  path,
  root,
  onPathSelect,
}: {
  path: string;
  root: Root;
  onPathSelect: (p: string) => void;
}) {
  const segments = path === "" ? [] : path.split("/");
  // Root segment is always present; subsequent segments are the path
  // pieces. The Breadcrumb component renders the last item as static
  // "you are here" automatically — so even though we attach onClick to
  // the deepest item, it won't fire.
  const items: BreadcrumbItem[] = [
    {
      label: root === "music" ? "Music" : "SFX",
      onClick: () => onPathSelect(""),
      title: root === "music" ? "Go to music root" : "Go to SFX root",
    },
    ...segments.map((name, i) => {
      const segmentPath = segments.slice(0, i + 1).join("/");
      return {
        label: name,
        onClick: () => onPathSelect(segmentPath),
        title: segmentPath,
      };
    }),
  ];

  // Load this folder (recursive) into the ambient queue. "" plays the whole
  // library. "Shuffle" flips shuffle to random first so the load is randomised;
  // both respect Continue (∞) for what happens once the folder is exhausted.
  const folderLabel = path === "" ? "all music" : segments[segments.length - 1];
  function playFolder(shuffled: boolean) {
    if (shuffled) wsClient.send({ type: "ambient_set_shuffle", shuffle: "random" });
    wsClient.send({ type: "ambient_play_folder", path });
  }

  return (
    <div className="folder-header">
      <Breadcrumb items={items} />
      {root === "music" ? (
        <div className="folder-header-actions">
          <IconButton
            label={`Play ${folderLabel}`}
            icon={<PlayIcon />}
            variant="primary"
            onClick={() => playFolder(false)}
          />
          <IconButton
            label={`Shuffle ${folderLabel}`}
            icon={<ShuffleIcon />}
            onClick={() => playFolder(true)}
          />
        </div>
      ) : null}
    </div>
  );
}

function FolderActions({
  root,
  selectedPath,
  onChanged,
  onPathReset,
}: {
  root: Root;
  selectedPath: string;
  onChanged: () => void;
  onPathReset: () => void;
}) {
  const [moveOpen, setMoveOpen] = useState(false);
  const [moveBusy, setMoveBusy] = useState(false);

  const loadAll = useCallback(
    () => (root === "music" ? musicAllFolders() : sfxAllFolders()),
    [root],
  );

  async function newFolder() {
    const name = await inputDialog({
      title: "New folder",
      body: `Will be created under ${selectedPath ? `"${selectedPath}"` : "the root"}.`,
      label: "Folder name",
      placeholder: "e.g. Skyrim",
      confirmLabel: "Create",
      // Reject path separators — operators create *one* folder at a time;
      // multi-level paths are unintuitive in a single-shot prompt.
      validate: (v) =>
        v.includes("/") || v.includes("\\") ? "No slashes — pick a nested folder, then create inside it." : null,
    });
    if (name === null) return;
    const target = selectedPath ? `${selectedPath}/${name}` : name;
    try {
      if (root === "music") await libraryApi.createFolder(target);
      else await sfxApi.createFolder(target);
      toast.success("Folder created", target);
      onChanged();
    } catch (e) {
      toast.error("Create failed", e instanceof Error ? e.message : undefined);
    }
  }

  async function renameFolder() {
    if (!selectedPath) {
      toast.info("Pick a folder first");
      return;
    }
    const parent = selectedPath.split("/").slice(0, -1).join("/");
    const current = selectedPath.split("/").pop() ?? selectedPath;
    const name = await inputDialog({
      title: "Rename folder",
      body: `Renaming "${selectedPath}". Use Move… to change its parent.`,
      label: "New name",
      initial: current,
      confirmLabel: "Rename",
      validate: (v) =>
        v.includes("/") || v.includes("\\")
          ? "No slashes — use Move… to change the parent folder."
          : null,
    });
    if (name === null || name === current) return;
    const target = parent ? `${parent}/${name}` : name;
    try {
      if (root === "music") await libraryApi.renameFolder(selectedPath, target);
      else await sfxApi.renameFolder(selectedPath, target);
      toast.success("Folder renamed", target);
      onChanged();
      onPathReset();
    } catch (e) {
      toast.error("Rename failed", e instanceof Error ? e.message : undefined);
    }
  }

  async function moveFolder(destParent: string) {
    const name = selectedPath.split("/").pop() ?? selectedPath;
    const target = destParent === "" ? name : `${destParent}/${name}`;
    if (target === selectedPath) {
      setMoveOpen(false);
      return;
    }
    setMoveBusy(true);
    try {
      if (root === "music") await libraryApi.renameFolder(selectedPath, target);
      else await sfxApi.renameFolder(selectedPath, target);
      toast.success("Folder moved", target);
      setMoveOpen(false);
      onChanged();
      onPathReset();
    } catch (e) {
      toast.error("Move failed", e instanceof Error ? e.message : undefined);
    } finally {
      setMoveBusy(false);
    }
  }

  async function deleteFolder() {
    if (!selectedPath) {
      toast.info("Pick a folder first");
      return;
    }
    const ok = await confirmDialog({
      title: `Delete "${selectedPath}"?`,
      body:
        "If the folder isn't empty, everything inside it will be removed too. This can't be undone.",
      confirmLabel: "Delete recursively",
      tone: "danger",
    });
    if (!ok) return;
    try {
      if (root === "music")
        await libraryApi.deleteFolder(selectedPath, true);
      else await sfxApi.deleteFolder(selectedPath, true);
      toast.success("Folder deleted", selectedPath);
      onChanged();
      onPathReset();
    } catch (e) {
      toast.error("Delete failed", e instanceof Error ? e.message : undefined);
    }
  }

  return (
    <>
      <div className="folder-actions">
        <IconButton
          label="Create a folder"
          icon={<PlusIcon />}
          onClick={() => void newFolder()}
        >
          New
        </IconButton>
        <IconButton
          label="Rename the selected folder"
          icon={<EditIcon />}
          disabled={!selectedPath}
          onClick={() => void renameFolder()}
        >
          Rename
        </IconButton>
        <IconButton
          label="Move the selected folder (and everything in it) into another folder"
          icon={<MoveIcon />}
          disabled={!selectedPath}
          onClick={() => setMoveOpen(true)}
        >
          Move…
        </IconButton>
        <IconButton
          label="Delete the selected folder and everything in it"
          icon={<TrashIcon />}
          variant="danger"
          className="folder-actions-delete"
          disabled={!selectedPath}
          onClick={() => void deleteFolder()}
        >
          Delete
        </IconButton>
      </div>
      {moveOpen ? (
        <FolderPickerModal
          title={`Move "${selectedPath}"`}
          body="Pick the destination parent folder. The folder and everything inside it moves together."
          loadAll={loadAll}
          busy={moveBusy}
          disableDest={(dest) =>
            dest === selectedPath || dest.startsWith(`${selectedPath}/`)
              ? "Can't move a folder into itself or one of its subfolders."
              : dest === selectedPath.split("/").slice(0, -1).join("/")
                ? "The folder is already here."
                : null
          }
          onCancel={() => setMoveOpen(false)}
          onConfirm={(dest) => void moveFolder(dest)}
        />
      ) : null}
    </>
  );
}

/** One unified band: the folder breadcrumb on the left, a compact Upload
 *  affordance on the right — and the WHOLE band is the drop target (drag
 *  files anywhere over it). Replaces the old two stacked full-width rows
 *  (breadcrumb row + separate drop-zone row). */
function FolderBand({
  root,
  dest,
  onPathSelect,
  onUploaded,
  onNavigate,
}: {
  root: Root;
  dest: string;
  onPathSelect: (p: string) => void;
  onUploaded: () => void;
  /** Navigate the browser to a folder path. Used after a folder drop so
   *  the operator lands inside the folder they just uploaded — otherwise
   *  the files vanish into a collapsed subfolder and it looks like the
   *  upload did nothing. */
  onNavigate: (path: string) => void;
}) {
  const [busy, setBusy] = useState(false);
  const [progress, setProgress] = useState<{ loaded: number; total: number } | null>(
    null,
  );
  const [dragOver, setDragOver] = useState(false);

  /** Build a single destination path under MUSIC_DIR / SFX_LIBRARY_DIR by
   *  joining the currently-selected folder with a relative sub-path that
   *  came from a dropped folder's structure. Empty sub-path means the
   *  drop's destination is just the current dest unchanged. */
  function joinDest(subPath: string): string {
    if (subPath === "") return dest;
    return dest === "" ? subPath : `${dest}/${subPath}`;
  }

  /** Run uploads for a list of collected files. Files at the same relative
   *  parent dir are uploaded together (single multipart request per dir);
   *  multiple parent dirs are uploaded sequentially so the progress bar
   *  reads as one monotonic operation across the whole drop.
   *
   *  Note: parallel uploads would be faster but make progress reporting
   *  fiddly and add server load. For a single-user home server with an
   *  occasional album drop, sequential is the right trade. */
  async function sendCollected(collected: CollectedFile[]) {
    if (collected.length === 0) {
      toast.warn(
        "Nothing to upload",
        "Drop contained no audio files (we accept mp3, flac, ogg, opus, m4a, aac, wav, wma).",
      );
      return;
    }
    setBusy(true);
    const totalBytes = collected.reduce((sum, c) => sum + c.file.size, 0);
    setProgress({ loaded: 0, total: totalBytes });

    const groups = groupByParent(collected);
    let baseLoaded = 0;
    let savedTotal = 0;

    // First top-level subfolder created by this drop (the first path
    // segment of any non-empty group key). Used to navigate the operator
    // into their freshly-uploaded folder afterwards. Null = drop was loose
    // files only, so we stay put (they appear in the current track table).
    let firstSubfolder: string | null = null;
    for (const subDir of groups.keys()) {
      if (subDir !== "") {
        firstSubfolder = subDir.split("/")[0] ?? null;
        break;
      }
    }

    try {
      for (const [subDir, files] of groups) {
        const groupDest = joinDest(subDir);
        const groupBytes = files.reduce((s, f) => s + f.size, 0);
        // libraryApi.upload reports (loaded, total) for that one group; we
        // re-base it against the cumulative drop total so the progress bar
        // moves monotonically across the whole multi-folder upload.
        const onGroupProgress = (loaded: number) => {
          setProgress({ loaded: baseLoaded + loaded, total: totalBytes });
        };
        if (root === "music") {
          const r = await libraryApi.upload(files, groupDest, onGroupProgress);
          savedTotal += r.saved.length;
        } else {
          const r = await sfxApi.upload(files, groupDest, onGroupProgress);
          savedTotal += r.saved.length;
        }
        baseLoaded += groupBytes;
      }
      const folderCount = groups.size;
      const detail =
        folderCount > 1
          ? `across ${folderCount} folders under "${dest || "(root)"}"`
          : `→ ${dest || "(root)"}`;
      toast.success(`Uploaded ${savedTotal}`, detail);
      // Always refresh (re-reads the index/tree so new folders + tracks
      // show up). For a folder drop, also navigate INTO the uploaded
      // folder so the result is immediately visible instead of hiding in
      // a collapsed subfolder — that's what made it look like "nothing
      // happened". Loose-file drops stay put; the files appear in the
      // current track table directly.
      onUploaded();
      if (firstSubfolder !== null) {
        onNavigate(joinDest(firstSubfolder));
      }
    } catch (e) {
      // Partial uploads happen — some files may have landed before the
      // failing request. Tell the operator both numbers so they know
      // whether to retry from scratch or just re-drop the missing folder.
      toast.error(
        "Upload failed",
        `${savedTotal} saved before error: ${e instanceof Error ? e.message : "unknown"}`,
      );
    } finally {
      setBusy(false);
      setProgress(null);
    }
  }

  // NOTE: this handler is intentionally NOT async. The DataTransfer and
  // the FileSystemEntry objects it yields are only valid during the
  // synchronous part of the drop event — the moment we `await`, the
  // browser (Firefox especially) neuters them and directory reads come
  // back empty. So we capture both the entry handles and the flat file
  // list synchronously here, then hand them to the async walker.
  function onDrop(e: DragEvent<HTMLDivElement>) {
    e.preventDefault();
    setDragOver(false);
    const entries = entriesFromItems(e.dataTransfer.items);
    const flatFiles = Array.from(e.dataTransfer.files);
    void processDrop(entries, flatFiles);
  }

  async function processDrop(entries: FileSystemEntry[], flatFiles: File[]) {
    // Prefer the entry walk (handles folders). Fall back to the flat file
    // list — captured synchronously above — when the entry API gave us
    // nothing (older browsers, programmatic drops, or a drop that somehow
    // produced no usable entries).
    let collected: CollectedFile[] = [];
    if (entries.length > 0) {
      try {
        collected = await collectEntries(entries);
      } catch {
        collected = [];
      }
    }
    if (collected.length === 0) {
      collected = flatFiles
        .filter((f) => isAudioFile(f.name))
        .map((file) => ({ relativePath: file.name, file }));
    }
    await sendCollected(collected);
  }
  function onPick(e: ChangeEvent<HTMLInputElement>) {
    // The file picker honours accept="audio/*" as a hint but the user can
    // still flip to "all files" — filter here so non-audio picks aren't
    // silently sent and stored as unindexed clutter.
    const collected: CollectedFile[] = Array.from(e.target.files ?? [])
      .filter((f) => isAudioFile(f.name))
      .map((file) => ({ relativePath: file.name, file }));
    e.target.value = "";
    void sendCollected(collected);
  }

  return (
    <div
      className={`folder-band${dragOver ? " folder-band-drop" : ""}${
        busy ? " folder-band-busy" : ""
      }`}
      onDragOver={(e) => {
        e.preventDefault();
        setDragOver(true);
      }}
      onDragLeave={(e) => {
        e.preventDefault();
        setDragOver(false);
      }}
      onDrop={onDrop}
    >
      <FolderHeader path={dest} root={root} onPathSelect={onPathSelect} />
      {busy && progress !== null ? (
        <div className="upload-progress folder-band-progress">
          <div className="upload-progress-label">
            Uploading — {formatSize(progress.loaded)} / {formatSize(progress.total)}
          </div>
          <progress
            aria-label="Upload progress"
            value={progress.total > 0 ? progress.loaded / progress.total : 0}
            max={1}
          />
        </div>
      ) : dragOver ? (
        <span className="folder-band-hint">
          Drop to upload to <strong>{dest === "" ? "(root)" : dest}</strong>
        </span>
      ) : (
        <label
          className="folder-band-upload"
          title={`Upload audio to ${dest === "" ? "(root)" : dest}`}
        >
          <input type="file" multiple accept="audio/*" onChange={onPick} hidden />
          <ImportIcon aria-hidden="true" />
          <span>Upload</span>
        </label>
      )}
    </div>
  );
}

function basename(p: string): string {
  return p.split("/").pop() || p;
}

/** Selection action bar above the track list — bulk move / delete of the
 *  ticked rows. Tag editing lives in the inspector, not here. Renders nothing
 *  until something is selected. */
function SelectionToolbar({
  selected,
  total,
  onClear,
  onChanged,
}: {
  selected: Set<number>;
  total: number;
  onClear: () => void;
  onChanged: () => void;
}) {
  const [moveOpen, setMoveOpen] = useState(false);
  const [busy, setBusy] = useState(false);
  const loadAll = useCallback(musicAllFolders, []);

  const ids = [...selected];

  function reportSkips(skipped: { track_id: number; reason: string }[]) {
    if (skipped.length === 0) return;
    const sample = skipped
      .slice(0, 3)
      .map((s) => `#${s.track_id}: ${s.reason}`)
      .join("\n");
    const more = skipped.length > 3 ? `\n…and ${skipped.length - 3} more` : "";
    toast.warn("Some tracks were skipped", `${sample}${more}`);
  }

  async function doMove(dest: string) {
    setBusy(true);
    try {
      const r = await libraryApi.bulkMove(ids, dest);
      toast.success(
        `Moved ${r.moved.length} of ${ids.length} track${ids.length === 1 ? "" : "s"}`,
      );
      reportSkips(r.skipped);
      setMoveOpen(false);
      onChanged();
    } catch (e) {
      toast.error("Move failed", e instanceof Error ? e.message : undefined);
    } finally {
      setBusy(false);
    }
  }

  async function doDelete() {
    const ok = await confirmDialog({
      title: "Delete selected tracks?",
      body:
        `Permanently delete ${ids.length} track${ids.length === 1 ? "" : "s"} from disk ` +
        "and the library? Playlist references will be removed too.",
      tone: "danger",
      confirmLabel: "Delete",
    });
    if (!ok) return;
    setBusy(true);
    try {
      const r = await libraryApi.bulkDelete(ids);
      toast.success(
        `Deleted ${r.deleted_ids.length} track${r.deleted_ids.length === 1 ? "" : "s"}`,
      );
      reportSkips(r.skipped);
      onChanged();
    } catch (e) {
      toast.error("Delete failed", e instanceof Error ? e.message : undefined);
    } finally {
      setBusy(false);
    }
  }

  if (ids.length === 0) return null;

  return (
    <>
      <div className="selection-bar">
        <span className="selection-bar-count">
          {ids.length} of {total} selected
        </span>
        <button type="button" disabled={busy} onClick={() => setMoveOpen(true)}>
          Move…
        </button>
        <button
          type="button"
          className="btn-danger"
          disabled={busy}
          onClick={() => void doDelete()}
        >
          Delete
        </button>
        <button type="button" className="btn-ghost" onClick={onClear}>
          Clear
        </button>
      </div>
      {moveOpen ? (
        <FolderPickerModal
          title={`Move ${ids.length} track${ids.length === 1 ? "" : "s"}`}
          body="Pick the destination folder under MUSIC_DIR. Files keep their names; collisions are skipped per-track."
          confirmVerb="Move"
          loadAll={loadAll}
          busy={busy}
          onCancel={() => setMoveOpen(false)}
          onConfirm={(dest) => void doMove(dest)}
        />
      ) : null}
    </>
  );
}

/** Multi-select track list — the list half of the unified Library. Columns are
 *  just Name (display) + File (basename) + Length; the rich tag fields live in
 *  the inspector.
 *
 *  Two separate gestures, never conflated:
 *   - A plain row-click only *highlights* the row (and feeds the inspector).
 *     Double-click plays. Neither ticks a checkbox.
 *   - Ticking a checkbox is the only way to build the bulk selection: a direct
 *     checkbox click, the header select-all, Ctrl/Cmd-click on a row, or a
 *     Shift-click range (anchor → clicked). */
function MusicTrackList({
  tracks,
  loading,
  activeTrackId,
  checked,
  onCheckedChange,
  focused,
  onFocusedChange,
  draggable,
  onChanged,
  onReveal,
}: {
  tracks: Track[];
  loading: boolean;
  activeTrackId: number | null;
  checked: Set<number>;
  onCheckedChange: (next: Set<number>) => void;
  focused: number | null;
  onFocusedChange: (id: number | null) => void;
  draggable?: boolean;
  onChanged: () => void;
  /** When set (search mode), rows offer a "show in folder" action that
   *  jumps back to browse mode at the track's parent folder. */
  onReveal?: (t: Track) => void;
}) {
  // Anchor for Shift-range selection: the last row the user single-clicked or
  // Ctrl-clicked. A ref (not state) — it only matters at the next click.
  const anchorRef = useRef<number | null>(null);

  // After reveal-from-search the freshly-loaded folder listing should show
  // the revealed track without hunting: nearest-scroll the focused row when
  // the visible set changes. (A plain click also lands here, but the row is
  // already in view then, so nearest-scroll is a no-op.)
  const rowEls = useRef(new Map<number, HTMLTableRowElement>());
  useEffect(() => {
    if (focused === null) return;
    rowEls.current.get(focused)?.scrollIntoView({ block: "nearest" });
  }, [tracks, focused]);

  if (tracks.length === 0) {
    // A folder fetch in flight must not flash the false "No tracks here"
    // empty state — mirror the search band's muted "Searching…" treatment.
    if (loading) {
      return <p className="muted small">Loading…</p>;
    }
    return (
      <EmptyState title="No tracks here">
        Drop audio files into the zone above, or pick a different folder from
        the tree on the left.
      </EmptyState>
    );
  }

  const allSelected = tracks.every((t) => checked.has(t.id));

  function toggleAll() {
    onCheckedChange(allSelected ? new Set() : new Set(tracks.map((t) => t.id)));
  }
  function toggleOne(id: number) {
    const next = new Set(checked);
    if (next.has(id)) next.delete(id);
    else next.add(id);
    anchorRef.current = id;
    onCheckedChange(next);
  }
  function selectRange(toId: number) {
    const fromId = anchorRef.current ?? focused;
    if (fromId === null) {
      toggleOne(toId);
      return;
    }
    const i = tracks.findIndex((t) => t.id === fromId);
    const j = tracks.findIndex((t) => t.id === toId);
    if (i === -1 || j === -1) {
      toggleOne(toId);
      return;
    }
    const [lo, hi] = i <= j ? [i, j] : [j, i];
    const next = new Set(checked);
    for (let k = lo; k <= hi; k++) next.add(tracks[k].id);
    onCheckedChange(next);
  }
  function onRowClick(e: MouseEvent<HTMLTableRowElement>, id: number) {
    const target = e.target as HTMLElement;
    if (target.closest(".col-actions") || target.closest(".col-check")) return;
    if (e.shiftKey) {
      selectRange(id);
      return;
    }
    if (e.ctrlKey || e.metaKey) {
      toggleOne(id);
      return;
    }
    // Plain click: highlight only — never ticks a checkbox.
    anchorRef.current = id;
    onFocusedChange(id);
  }

  function play(t: Track) {
    wsClient.send({ type: "ambient_play_track", track_id: t.id });
  }
  function enqueue(t: Track) {
    wsClient.send({ type: "ambient_enqueue", track_id: t.id });
  }
  function fireInterrupt(t: Track) {
    wsClient.send({
      type: "fire_interrupt_track",
      track_id: t.id,
      return_to_ambient: true,
      fade_in_ms: 500,
      fade_out_ms: 500,
    });
    toast.info("Interrupt fired", trackTitle(t));
  }
  async function deleteTrack(t: Track) {
    const ok = await confirmDialog({
      title: "Delete track?",
      body: `${t.path} will be permanently removed from disk.`,
      tone: "danger",
      confirmLabel: "Delete",
    });
    if (!ok) return;
    try {
      await libraryApi.deleteTrack(t.id);
      toast.success("Deleted");
      onChanged();
    } catch (e) {
      toast.error("Delete failed", e instanceof Error ? e.message : undefined);
    }
  }

  return (
    <div className="track-table-wrap">
      <table className="track-table">
        <thead>
          <tr>
            <th className="col-check">
              <input
                type="checkbox"
                checked={allSelected}
                onChange={toggleAll}
                aria-label="Select all"
              />
            </th>
            <th>Name</th>
            <th>File</th>
            <th className="col-num">Length</th>
            <th className="col-actions" />
          </tr>
        </thead>
        <tbody>
          {tracks.map((t) => {
            const isChecked = checked.has(t.id);
            const isFocused = focused === t.id;
            return (
              <tr
                key={t.id}
                ref={(el) => {
                  if (el) rowEls.current.set(t.id, el);
                  else rowEls.current.delete(t.id);
                }}
                className={`track-row${isChecked ? " checked" : ""}${
                  isFocused ? " focused" : ""
                }${activeTrackId === t.id ? " playing" : ""}`}
                onClick={(e) => onRowClick(e, t.id)}
                onDoubleClick={() => play(t)}
                draggable={draggable}
                onDragStart={
                  draggable
                    ? (e) => {
                        e.dataTransfer.setData(
                          "application/json",
                          JSON.stringify({ kind: "music-track", id: t.id }),
                        );
                        e.dataTransfer.effectAllowed = "move";
                      }
                    : undefined
                }
              >
                <td className="col-check">
                  <input
                    type="checkbox"
                    checked={isChecked}
                    onChange={() => toggleOne(t.id)}
                    aria-label={`Select ${trackTitle(t)}`}
                  />
                </td>
                <td title={t.path}>{trackTitle(t)}</td>
                <td className="track-file muted">{basename(t.path)}</td>
                <td className="col-num">{formatDuration(t.length_s)}</td>
                <td className="col-actions">
                  {onReveal ? (
                    <IconButton
                      label="Show in folder"
                      icon={<FolderOpenIcon />}
                      onClick={() => onReveal(t)}
                    />
                  ) : null}
                  <IconButton label="Play" icon={<PlayIcon />} onClick={() => play(t)} />
                  <IconButton
                    label="Add to queue"
                    icon={<PlusIcon />}
                    onClick={() => enqueue(t)}
                  />
                  <IconButton
                    label="Fire as interrupt (overrides ambient until done, then resumes)"
                    icon={<LightningIcon />}
                    onClick={() => fireInterrupt(t)}
                  />
                  <IconButton
                    label="Delete track"
                    icon={<TrashIcon />}
                    variant="danger"
                    onClick={() => void deleteTrack(t)}
                  />
                </td>
              </tr>
            );
          })}
        </tbody>
      </table>
    </div>
  );
}
