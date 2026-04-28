import { useCallback, useEffect, useMemo, useState } from "react";
import type { FormEvent } from "react";

import { libraryApi } from "@/core/api";
import type { LibrarySortKey, SearchResponse, SortOrder } from "@/core/api";
import { usePlayerStore, selectActiveTrackId } from "@/core/playerStore";
import { toast } from "@/core/toast";
import type { FolderEntry, Track } from "@/core/types";
import { wsClient } from "@/core/ws";
import { confirmDialog } from "@/components/ConfirmDialog";
import { MetadataEditor } from "@/components/MetadataEditor";
import { MoveDialog } from "@/components/MoveDialog";
import { UploadDropZone } from "@/components/UploadDropZone";

const PAGE_SIZE = 100;
type Mode = "browse" | "search";

interface ColumnDef {
  key: LibrarySortKey;
  label: string;
  render: (t: Track) => string | number | null | undefined;
  className?: string;
}

const COLUMNS: ColumnDef[] = [
  { key: "title", label: "Title", render: (t) => t.title || t.path },
  { key: "artist", label: "Artist", render: (t) => t.artist },
  { key: "album", label: "Album", render: (t) => t.album },
  { key: "year", label: "Year", render: (t) => t.year, className: "col-num" },
  { key: "track_no", label: "#", render: (t) => t.track_no, className: "col-num" },
  {
    key: "length_s",
    label: "Length",
    render: (t) => formatDuration(t.length_s),
    className: "col-num",
  },
];

function formatDuration(seconds: number): string {
  if (!seconds || seconds <= 0) return "—";
  const total = Math.floor(seconds);
  const m = Math.floor(total / 60);
  const s = total % 60;
  return `${m}:${s.toString().padStart(2, "0")}`;
}

export function LibraryView() {
  const [mode, setMode] = useState<Mode>("browse");
  const [folder, setFolder] = useState<string>("");
  const [folderTracks, setFolderTracks] = useState<Track[]>([]);
  const [folderChildren, setFolderChildren] = useState<FolderEntry[]>([]);
  const [breadcrumbs, setBreadcrumbs] = useState<string[]>([]);

  const [pendingQuery, setPendingQuery] = useState("");
  const [query, setQuery] = useState("");
  const [sort, setSort] = useState<LibrarySortKey>("artist");
  const [order, setOrder] = useState<SortOrder>("asc");
  const [offset, setOffset] = useState(0);
  const [searchResponse, setSearchResponse] = useState<SearchResponse | null>(null);

  const [loading, setLoading] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const [editing, setEditing] = useState<Track | null>(null);
  const [moving, setMoving] = useState<Track | null>(null);
  const [knownFolders, setKnownFolders] = useState<string[]>([]);

  const activeTrackId = usePlayerStore(selectActiveTrackId);

  // --- data loading ------------------------------------------------------

  const loadFolder = useCallback(async (path: string) => {
    setLoading(true);
    setError(null);
    try {
      const res = await libraryApi.tree(path);
      setFolderTracks(res.tracks);
      setFolderChildren(res.folders);
      setBreadcrumbs(res.path === "" ? [] : res.path.split("/"));
    } catch (e) {
      setError(e instanceof Error ? e.message : "load failed");
      setFolderTracks([]);
      setFolderChildren([]);
    } finally {
      setLoading(false);
    }
  }, []);

  const runSearch = useCallback(
    async (q: string, opts: { sort: LibrarySortKey; order: SortOrder; offset: number }) => {
      setLoading(true);
      setError(null);
      try {
        const res = await libraryApi.search({
          q,
          limit: PAGE_SIZE,
          offset: opts.offset,
          sort: opts.sort,
          order: opts.order,
        });
        setSearchResponse(res);
      } catch (e) {
        setError(e instanceof Error ? e.message : "search failed");
        setSearchResponse(null);
      } finally {
        setLoading(false);
      }
    },
    [],
  );

  useEffect(() => {
    if (mode === "browse") {
      void loadFolder(folder);
    } else {
      void runSearch(query, { sort, order, offset });
    }
  }, [mode, folder, query, sort, order, offset, loadFolder, runSearch]);

  // Walk the tree on mount once to build the folder dropdown for MoveDialog.
  useEffect(() => {
    let cancelled = false;
    async function walk(path: string, acc: string[]) {
      const res = await libraryApi.tree(path);
      for (const f of res.folders) {
        acc.push(f.path);
        await walk(f.path, acc);
      }
    }
    const collected: string[] = [];
    void walk("", collected).then(() => {
      if (!cancelled) setKnownFolders(collected);
    });
    return () => {
      cancelled = true;
    };
  }, []);

  // --- handlers ----------------------------------------------------------

  function onSubmitSearch(e: FormEvent) {
    e.preventDefault();
    setMode("search");
    setOffset(0);
    setQuery(pendingQuery);
  }

  function clearSearch() {
    setPendingQuery("");
    setQuery("");
    setOffset(0);
    setMode("browse");
  }

  function onClickSort(key: LibrarySortKey) {
    if (sort === key) setOrder(order === "asc" ? "desc" : "asc");
    else {
      setSort(key);
      setOrder("asc");
    }
    setOffset(0);
  }

  function navigateTo(path: string) {
    setMode("browse");
    setFolder(path);
  }

  function navigateUp() {
    if (breadcrumbs.length === 0) return;
    navigateTo(breadcrumbs.slice(0, -1).join("/"));
  }

  function play(track: Track) {
    wsClient.send({ type: "ambient_play_track", track_id: track.id });
  }

  function enqueue(track: Track) {
    wsClient.send({ type: "ambient_enqueue", track_id: track.id });
  }

  function fireAsInterrupt(track: Track) {
    wsClient.send({ type: "fire_interrupt_track", track_id: track.id });
  }

  function playFolder() {
    if (folderTracks.length === 0) return;
    const ids = folderTracks.map((t) => t.id);
    wsClient.send({ type: "ambient_set_queue", track_ids: ids.slice(1) });
    wsClient.send({ type: "ambient_play_track", track_id: ids[0] });
  }

  async function deleteTrack(track: Track) {
    const ok = await confirmDialog({
      title: "Delete track?",
      body: `${track.path} will be permanently removed from disk. This can't be undone.`,
      confirmLabel: "Delete",
      cancelLabel: "Keep",
      tone: "danger",
    });
    if (!ok) return;
    try {
      await libraryApi.deleteTrack(track.id);
      toast.success("Deleted", track.path);
      if (mode === "browse") void loadFolder(folder);
      else void runSearch(query, { sort, order, offset });
    } catch (e) {
      toast.error("Delete failed", e instanceof Error ? e.message : undefined);
    }
  }

  async function rescan() {
    setLoading(true);
    try {
      const result = await libraryApi.rescan();
      const parts: string[] = [];
      if (result.added) parts.push(`+${result.added} added`);
      if (result.updated) parts.push(`${result.updated} updated`);
      if (result.removed) parts.push(`-${result.removed} removed`);
      toast.success(
        "Rescan complete",
        parts.length === 0 ? "No changes." : parts.join(", "),
      );
      if (mode === "browse") await loadFolder(folder);
      else await runSearch(query, { sort, order, offset });
    } catch (e) {
      toast.error("Rescan failed", e instanceof Error ? e.message : undefined);
    } finally {
      setLoading(false);
    }
  }

  // --- rendering ---------------------------------------------------------

  const tracksToShow: Track[] =
    mode === "search" ? searchResponse?.tracks ?? [] : folderTracks;

  const headerCells = useMemo(
    () =>
      COLUMNS.map((col) => {
        const sortable = mode === "search";
        const active = sort === col.key;
        const indicator = sortable && active ? (order === "asc" ? "▲" : "▼") : "";
        return (
          <th
            key={col.key}
            className={`${sortable ? "sortable" : ""} ${col.className ?? ""} ${
              active ? "sort-active" : ""
            }`}
            onClick={sortable ? () => onClickSort(col.key) : undefined}
            scope="col"
          >
            <span>{col.label}</span>
            <span className="sort-indicator">{indicator}</span>
          </th>
        );
      }),
    // eslint-disable-next-line react-hooks/exhaustive-deps
    [mode, sort, order],
  );

  const total = mode === "search" ? searchResponse?.total ?? 0 : folderTracks.length;
  const showingFrom = mode === "search" ? (total === 0 ? 0 : offset + 1) : 1;
  const showingTo =
    mode === "search"
      ? Math.min(offset + tracksToShow.length, total)
      : tracksToShow.length;
  const canPrev = mode === "search" && offset > 0;
  const canNext =
    mode === "search" &&
    searchResponse !== null &&
    offset + searchResponse.tracks.length < searchResponse.total;

  return (
    <div className="library-view">
      <UploadDropZone
        defaultDest={mode === "browse" && folder !== "" ? folder : "Uploads"}
        onUploaded={(_tracks, _destination) => {
          if (mode === "browse") void loadFolder(folder);
          else void runSearch(query, { sort, order, offset });
        }}
      />
      <header className="library-toolbar">
        <form onSubmit={onSubmitSearch} className="library-search">
          <input
            type="search"
            value={pendingQuery}
            onChange={(e) => setPendingQuery(e.target.value)}
            placeholder="Search title / artist / album / path"
          />
          <button type="submit" disabled={loading}>
            Search
          </button>
          {mode === "search" ? (
            <button type="button" onClick={clearSearch}>
              Browse
            </button>
          ) : null}
          <button type="button" onClick={() => void rescan()} disabled={loading} title="Rescan MUSIC_DIR for changes">
            Rescan
          </button>
        </form>

        {error !== null ? <p className="error small">{error}</p> : null}
      </header>

      <div className="library-body">
        <aside className="library-sidebar">
          <h3 className="muted small">Folders</h3>
          <div className="library-folder-list">
            <button
              type="button"
              className={`library-folder ${
                mode === "browse" && breadcrumbs.length === 0 ? "active" : ""
              }`}
              onClick={() => navigateTo("")}
            >
              <span>↑ /</span>
              <span className="muted small">all</span>
            </button>
            {mode === "browse" && breadcrumbs.length > 0 ? (
              <button type="button" className="library-folder" onClick={navigateUp}>
                <span>← up</span>
              </button>
            ) : null}
            {folderChildren.map((f) => (
              <button
                key={f.path}
                type="button"
                className="library-folder"
                onClick={() => navigateTo(f.path)}
              >
                <span>📁 {f.name}</span>
                <span className="muted small">{f.track_count}</span>
              </button>
            ))}
          </div>
        </aside>

        <section className="library-main">
          {mode === "browse" ? (
            <div className="library-meta">
              <span className="muted small">
                {breadcrumbs.length === 0 ? "/" : breadcrumbs.join(" / ")} —{" "}
                {tracksToShow.length} track
                {tracksToShow.length === 1 ? "" : "s"}
              </span>
              <button type="button" onClick={playFolder} disabled={tracksToShow.length === 0}>
                ▶ Play folder
              </button>
            </div>
          ) : (
            <div className="library-meta">
              {loading ? (
                <span className="muted small">Loading…</span>
              ) : (
                <span className="muted small">
                  {total === 0 ? "No matches." : `${showingFrom}–${showingTo} of ${total}`}
                </span>
              )}
              <div className="library-pager">
                <button
                  type="button"
                  onClick={() => setOffset(Math.max(0, offset - PAGE_SIZE))}
                  disabled={!canPrev || loading}
                >
                  ← Prev
                </button>
                <button
                  type="button"
                  onClick={() => setOffset(offset + PAGE_SIZE)}
                  disabled={!canNext || loading}
                >
                  Next →
                </button>
              </div>
            </div>
          )}

          {tracksToShow.length > 0 ? (
            <div className="track-table-wrap">
              <table className="track-table">
                <thead>
                  <tr>
                    {headerCells}
                    <th className="col-actions" />
                  </tr>
                </thead>
                <tbody>
                  {tracksToShow.map((t) => {
                    const isPlaying = activeTrackId === t.id;
                    return (
                      <tr
                        key={t.id}
                        className={`track-row ${isPlaying ? "playing" : ""}`}
                        onDoubleClick={() => play(t)}
                      >
                        {COLUMNS.map((col) => {
                          const v = col.render(t);
                          const display =
                            v === null || v === undefined || v === "" ? "—" : v;
                          const isMuted = display === "—";
                          return (
                            <td
                              key={col.key}
                              className={`${col.className ?? ""} ${
                                isMuted ? "muted" : ""
                              }`}
                            >
                              {display}
                            </td>
                          );
                        })}
                        <td className="col-actions">
                          <button onClick={() => play(t)} title="Play now">
                            ▶
                          </button>
                          <button onClick={() => enqueue(t)} title="Queue">
                            ＋
                          </button>
                          <button onClick={() => fireAsInterrupt(t)} title="Fire as interrupt">
                            ⚡
                          </button>
                          <button onClick={() => setEditing(t)} title="Edit metadata">
                            ✎
                          </button>
                          <button onClick={() => setMoving(t)} title="Move / rename">
                            ↪
                          </button>
                          <button
                            className="btn-danger"
                            onClick={() => void deleteTrack(t)}
                            title="Delete from disk"
                          >
                            🗑
                          </button>
                        </td>
                      </tr>
                    );
                  })}
                </tbody>
              </table>
            </div>
          ) : !loading ? (
            <div className="library-empty">
              {mode === "browse" ? (
                <>
                  <h3>Nothing in this folder</h3>
                  <p className="muted small">
                    Drop files into the upload area at the top — or pick a
                    different folder on the left. Files added directly via SFTP
                    appear after a <button
                      type="button"
                      className="btn-ghost inline"
                      onClick={() => void rescan()}
                    >
                      Rescan
                    </button>.
                  </p>
                </>
              ) : (
                <>
                  <h3>No tracks match "{query}"</h3>
                  <p className="muted small">
                    Search runs against title, artist, album, and path. Press{" "}
                    <kbd>/</kbd> to focus the search box.
                  </p>
                </>
              )}
            </div>
          ) : null}
        </section>
      </div>

      {editing !== null ? (
        <MetadataEditor
          track={editing}
          onClose={() => setEditing(null)}
          onSaved={(updated) => {
            // refresh the relevant track in-place
            if (mode === "browse") {
              setFolderTracks((rows) =>
                rows.map((r) => (r.id === updated.id ? updated : r)),
              );
            } else {
              setSearchResponse((res) =>
                res === null
                  ? res
                  : {
                      ...res,
                      tracks: res.tracks.map((r) =>
                        r.id === updated.id ? updated : r,
                      ),
                    },
              );
            }
          }}
        />
      ) : null}

      {moving !== null ? (
        <MoveDialog
          track={moving}
          knownFolders={knownFolders}
          onClose={() => setMoving(null)}
          onMoved={(updated) => {
            // After a move, the track may no longer be in the current folder
            // view — easier to re-fetch.
            if (mode === "browse") void loadFolder(folder);
            else
              setSearchResponse((res) =>
                res === null
                  ? res
                  : {
                      ...res,
                      tracks: res.tracks.map((r) =>
                        r.id === updated.id ? updated : r,
                      ),
                    },
              );
          }}
        />
      ) : null}
    </div>
  );
}
