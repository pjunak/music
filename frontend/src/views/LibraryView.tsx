import { useCallback, useEffect, useState } from "react";
import type { ChangeEvent, DragEvent } from "react";

import { Breadcrumb } from "@/components/Breadcrumb";
import type { BreadcrumbItem } from "@/components/Breadcrumb";
import { confirmDialog } from "@/components/confirmDialog";
import { EmptyState } from "@/components/EmptyState";
import { FolderTree } from "@/components/FolderTree";
import type { TreeFolder } from "@/components/FolderTree";
import { IconButton } from "@/components/IconButton";
import {
  EditIcon,
  LightningIcon,
  PlayIcon,
  PlusIcon,
  TrashIcon,
} from "@/components/icons";
import { inputDialog } from "@/components/inputDialog";
import { MetadataEditor } from "@/components/MetadataEditor";
import { libraryApi, sfxApi } from "@/core/api";
import type { SfxFile } from "@/core/api";
import { selectActiveTrackId, usePlayerStore } from "@/core/playerStore";
import { toast } from "@/core/toast";
import { trackTitle } from "@/core/trackDisplay";
import type { Track } from "@/core/types";
import { useDebouncedValue } from "@/core/useDebouncedValue";
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

export function LibraryView() {
  const [root, setRoot] = useState<Root>("music");
  const [path, setPath] = useState("");
  const [refreshKey, setRefreshKey] = useState(0);
  const [pendingQuery, setPendingQuery] = useState("");
  const [query, setQuery] = useState("");

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
  }

  return (
    <div className="library-view">
      <header className="library-toolbar">
        <div className="library-root-toggle" role="tablist" aria-label="Library root">
          <button
            type="button"
            role="tab"
            aria-selected={root === "music"}
            className={root === "music" ? "btn-primary" : ""}
            onClick={() => selectRoot("music")}
          >
            🎵 Music
          </button>
          <button
            type="button"
            role="tab"
            aria-selected={root === "sfx"}
            className={root === "sfx" ? "btn-primary" : ""}
            onClick={() => selectRoot("sfx")}
          >
            ⚡ SFX
          </button>
        </div>
        {root === "music" ? (
          <div className="library-toolbar-search">
            <span className="library-toolbar-search-icon" aria-hidden="true">
              🔍
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
                ✕
              </button>
            ) : null}
          </div>
        ) : null}
        <RescanButton root={root} onComplete={() => setRefreshKey((k) => k + 1)} />
      </header>

      {root === "music" && query ? (
        <MusicSearchResults query={query} refreshKey={refreshKey} />
      ) : root === "music" ? (
        <MusicBrowser
          path={path}
          onPathChange={setPath}
          refreshKey={refreshKey}
          onRefresh={() => setRefreshKey((k) => k + 1)}
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
      <button type="button" onClick={onComplete} disabled={busy}>
        ↻ Refresh
      </button>
    );
  }
  return (
    <button
      type="button"
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
      {busy ? "Rescanning…" : "↻ Rescan"}
    </button>
  );
}

// --- MUSIC ---------------------------------------------------------------

function MusicSearchResults({
  query,
  refreshKey,
}: {
  query: string;
  refreshKey: number;
}) {
  const [tracks, setTracks] = useState<Track[]>([]);
  const [loading, setLoading] = useState(false);
  const activeTrackId = usePlayerStore(selectActiveTrackId);

  useEffect(() => {
    let cancelled = false;
    setLoading(true);
    void libraryApi
      .search({ q: query, limit: 200, sort: "artist", order: "asc" })
      .then((r) => {
        if (!cancelled) setTracks(r.tracks);
      })
      .catch(() => {
        if (!cancelled) setTracks([]);
      })
      .finally(() => {
        if (!cancelled) setLoading(false);
      });
    return () => {
      cancelled = true;
    };
  }, [query, refreshKey]);

  return (
    <div className="library-body library-body-search">
      <div className="library-main">
        <p className="muted small">
          {loading
            ? "Searching…"
            : `${tracks.length} match${tracks.length === 1 ? "" : "es"}`}
        </p>
        <TrackTable tracks={tracks} activeTrackId={activeTrackId} onChanged={() => undefined} />
      </div>
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
  loadChildren,
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
  loadChildren: (path: string) => Promise<TreeFolder[]>;
  onDropOnFolder?: (folderPath: string, payload: unknown) => void;
  onRefresh: () => void;
  onPathReset: () => void;
  error: string | null;
  children: React.ReactNode;
}) {
  return (
    <div className="library-body">
      <aside className="library-sidebar">
        <FolderTree
          selectedPath={selectedPath}
          onSelect={onPathChange}
          loadChildren={loadChildren}
          {...(refreshKey !== undefined ? { refreshKey } : {})}
          {...(onDropOnFolder !== undefined ? { onDropOnFolder } : {})}
        />
        <FolderActions
          root={rootKind}
          selectedPath={selectedPath}
          onChanged={onRefresh}
          onPathReset={onPathReset}
        />
      </aside>
      <section className="library-main">
        <FolderHeader
          path={selectedPath}
          root={rootKind}
          onPathSelect={onPathChange}
        />
        <UploadDrop root={rootKind} dest={selectedPath} onUploaded={onRefresh} />
        {error !== null ? <p className="error small">{error}</p> : null}
        {children}
      </section>
    </div>
  );
}

function MusicBrowser({
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
  const activeTrackId = usePlayerStore(selectActiveTrackId);
  const [tracks, setTracks] = useState<Track[]>([]);
  const [error, setError] = useState<string | null>(null);

  useEffect(() => {
    let cancelled = false;
    setError(null);
    void libraryApi
      .tree(path)
      .then((r) => {
        if (!cancelled) setTracks(r.tracks);
      })
      .catch((e: unknown) => {
        if (!cancelled) {
          setError(e instanceof Error ? e.message : "load failed");
          setTracks([]);
        }
      });
    return () => {
      cancelled = true;
    };
  }, [path, refreshKey]);

  const loadChildren = useCallback(async (p: string): Promise<TreeFolder[]> => {
    const r = await libraryApi.tree(p);
    return r.folders.map((f) => ({
      name: f.name,
      path: f.path,
      badge: f.track_count > 0 ? f.track_count : null,
      hasChildren: f.has_children,
    }));
  }, []);

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

  return (
    <LibraryShell
      rootKind="music"
      selectedPath={path}
      onPathChange={onPathChange}
      refreshKey={refreshKey}
      loadChildren={loadChildren}
      onDropOnFolder={onDropOnFolder}
      onRefresh={onRefresh}
      onPathReset={() => onPathChange("")}
      error={error}
    >
      <TrackTable
        tracks={tracks}
        activeTrackId={activeTrackId}
        onChanged={onRefresh}
        draggable
      />
    </LibraryShell>
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

  const loadChildren = useCallback(async (p: string): Promise<TreeFolder[]> => {
    const r = await sfxApi.tree(p);
    return r.folders.map((f) => ({
      name: f.name,
      path: f.path,
      badge: f.file_count > 0 ? f.file_count : null,
      hasChildren: f.has_children,
    }));
  }, []);

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
      loadChildren={loadChildren}
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
                  <IconButton label="Preview SFX" icon={<PlayIcon />} onClick={() => preview(f)} />
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
      label: root === "music" ? "🎵 Music" : "⚡ SFX",
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
  return (
    <div className="folder-header">
      <Breadcrumb items={items} />
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
    const next = await inputDialog({
      title: "Rename or move folder",
      body: `Current path: ${selectedPath}. Use slashes to nest under a different parent.`,
      label: "New path",
      initial: selectedPath,
      confirmLabel: "Rename",
    });
    if (next === null || next === selectedPath) return;
    try {
      if (root === "music") await libraryApi.renameFolder(selectedPath, next);
      else await sfxApi.renameFolder(selectedPath, next);
      toast.success("Folder renamed", next);
      onChanged();
      onPathReset();
    } catch (e) {
      toast.error("Rename failed", e instanceof Error ? e.message : undefined);
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
    <div className="folder-actions">
      <IconButton
        label="New folder"
        icon={<PlusIcon />}
        onClick={() => void newFolder()}
      >
        Folder
      </IconButton>
      <IconButton
        label="Rename / move the selected folder"
        icon={<EditIcon />}
        onClick={() => void renameFolder()}
        disabled={!selectedPath}
      >
        Rename
      </IconButton>
      <IconButton
        label="Delete the selected folder"
        icon={<TrashIcon />}
        variant="danger"
        onClick={() => void deleteFolder()}
        disabled={!selectedPath}
      >
        Delete
      </IconButton>
    </div>
  );
}

function UploadDrop({
  root,
  dest,
  onUploaded,
}: {
  root: Root;
  dest: string;
  onUploaded: () => void;
}) {
  const [busy, setBusy] = useState(false);
  const [progress, setProgress] = useState<{ loaded: number; total: number } | null>(
    null,
  );
  const [dragOver, setDragOver] = useState(false);

  async function send(files: File[]) {
    if (files.length === 0) return;
    setBusy(true);
    setProgress({ loaded: 0, total: 0 });
    try {
      if (root === "music") {
        const result = await libraryApi.upload(files, dest, (loaded, total) =>
          setProgress({ loaded, total }),
        );
        toast.success(
          `Uploaded ${result.saved.length}`,
          `→ ${result.destination || "(root)"}`,
        );
      } else {
        const result = await sfxApi.upload(files, dest, (loaded, total) =>
          setProgress({ loaded, total }),
        );
        toast.success(
          `Uploaded ${result.saved.length}`,
          `→ ${result.destination || "(root)"}`,
        );
      }
      onUploaded();
    } catch (e) {
      toast.error("Upload failed", e instanceof Error ? e.message : undefined);
    } finally {
      setBusy(false);
      setProgress(null);
    }
  }

  function onDrop(e: DragEvent<HTMLLabelElement>) {
    e.preventDefault();
    setDragOver(false);
    void send(Array.from(e.dataTransfer.files));
  }
  function onPick(e: ChangeEvent<HTMLInputElement>) {
    void send(Array.from(e.target.files ?? []));
    e.target.value = "";
  }

  return (
    <label
      className={`drop-zone${dragOver ? " drop-zone-active" : ""}${
        busy ? " drop-zone-uploading" : ""
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
      <input type="file" multiple accept="audio/*" onChange={onPick} hidden />
      {busy && progress !== null ? (
        <div className="upload-progress">
          <div className="upload-progress-label">
            Uploading — {formatSize(progress.loaded)} / {formatSize(progress.total)}
          </div>
          <progress
            value={progress.total > 0 ? progress.loaded / progress.total : 0}
            max={1}
          />
        </div>
      ) : (
        <span>
          Drop audio here or click to upload to{" "}
          <strong>{dest === "" ? "(root)" : dest}</strong>
        </span>
      )}
    </label>
  );
}

function TrackTable({
  tracks,
  activeTrackId,
  onChanged,
  draggable,
}: {
  tracks: Track[];
  activeTrackId: number | null;
  onChanged: () => void;
  draggable?: boolean;
}) {
  const [editing, setEditing] = useState<Track | null>(null);

  if (tracks.length === 0) {
    return (
      <EmptyState title="No tracks in this folder">
        Drop audio files into the zone above, or pick a different folder
        from the tree on the left.
      </EmptyState>
    );
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
    <>
      <div className="track-table-wrap">
        <table className="track-table">
          <thead>
            <tr>
              <th>Title</th>
              <th>Artist</th>
              <th>Album</th>
              <th className="col-num">Year</th>
              <th className="col-num">Length</th>
              <th className="col-actions" />
            </tr>
          </thead>
          <tbody>
            {tracks.map((t) => (
              <tr
                key={t.id}
                className={`track-row ${activeTrackId === t.id ? "playing" : ""}`}
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
                <td title={t.path}>{trackTitle(t)}</td>
                <td>{t.artist || <span className="muted">—</span>}</td>
                <td>{t.album || <span className="muted">—</span>}</td>
                <td className="col-num">{t.year ?? <span className="muted">—</span>}</td>
                <td className="col-num">{formatDuration(t.length_s)}</td>
                <td className="col-actions">
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
                    label="Edit metadata"
                    icon={<EditIcon />}
                    onClick={() => setEditing(t)}
                  />
                  <IconButton
                    label="Delete track"
                    icon={<TrashIcon />}
                    variant="danger"
                    onClick={() => void deleteTrack(t)}
                  />
                </td>
              </tr>
            ))}
          </tbody>
        </table>
      </div>
      {editing !== null ? (
        <MetadataEditor
          track={editing}
          onClose={() => setEditing(null)}
          onSaved={() => {
            setEditing(null);
            onChanged();
          }}
        />
      ) : null}
    </>
  );
}
