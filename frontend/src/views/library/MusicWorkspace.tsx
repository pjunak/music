import { useCallback, useEffect, useMemo, useRef, useState } from "react";
import type { Dispatch, MouseEvent, SetStateAction } from "react";

import { confirmDialog } from "@/components/confirmDialog";
import { EmptyState } from "@/components/EmptyState";
import { FolderPickerModal } from "@/components/FolderPickerModal";
import { FolderTree } from "@/components/FolderTree";
import { IconButton } from "@/components/IconButton";
import { LibrarySidebarRail } from "@/components/LibrarySidebarRail";
import { FolderOpenIcon, LightningIcon, PlayIcon, PlusIcon, TrashIcon } from "@/components/icons";
import { TrackMetadataDetails } from "@/components/TrackMetadataDetails";
import { TagInspector } from "@/components/TagInspector";
import { libraryApi } from "@/core/api";
import { selectActiveTrackId, usePlayerStore } from "@/core/playerStore";
import { toast } from "@/core/toast";
import { trackTitle } from "@/core/trackDisplay";
import type { Track } from "@/core/types";
import { wsClient } from "@/core/ws";

import { formatDuration, musicAllFolders, basename } from "./shared";
import { followConversion } from "./conversion";
import { FolderActions } from "./FolderHeader";
import { FolderBand } from "./FolderUploadBand";

/** The unified music screen: wide folder tree (browse mode) + a multi-select
 *  Name/File track list + a right-hand TagInspector that edits the selection.
 *  Search and browse share the same list + inspector; search just drops the
 *  tree + upload zone (a search is already a global filter). */
export function MusicWorkspace({
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

  const loadAll = useCallback(() => musicAllFolders(), []);

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
        <LibrarySidebarRail>
          <FolderTree
            selectedPath={path}
            onSelect={onPathChange}
            loadAll={loadAll}
            refreshKey={refreshKey}
            onDropOnFolder={(folderPath, payload) => void onDropOnFolder(folderPath, payload)}
          />
          <FolderActions
            root="music"
            selectedPath={path}
            onChanged={onRefresh}
            onPathReset={() => onPathChange("")}
          />
        </LibrarySidebarRail>
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
  const loadAll = useCallback(() => musicAllFolders(), []);

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

  async function doConvert() {
    setBusy(true);
    try {
      const job = await libraryApi.convert(ids);
      toast.info(
        `Converting ${ids.length} track${ids.length === 1 ? "" : "s"}`,
        "Lossless and high-bitrate files become 192 kbps AAC; originals are kept.",
      );
      void followConversion(job.id, onChanged);
    } catch (e) {
      toast.error("Conversion did not start", e instanceof Error ? e.message : undefined);
    } finally {
      setBusy(false);
    }
  }

  async function doRestore() {
    setBusy(true);
    try {
      const r = await libraryApi.bulkRestore(ids);
      toast.success(
        `Restored ${r.restored.length} original${r.restored.length === 1 ? "" : "s"}`,
      );
      reportSkips(r.skipped);
      onChanged();
    } catch (e) {
      toast.error("Restore failed", e instanceof Error ? e.message : undefined);
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
          disabled={busy}
          title="Convert lossless and high-bitrate tracks to 192 kbps AAC, keeping the originals"
          onClick={() => void doConvert()}
        >
          Convert
        </button>
        <button
          type="button"
          disabled={busy}
          title="Put the kept originals of converted tracks back in the library"
          onClick={() => void doRestore()}
        >
          Restore originals
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
            <th className="col-actions" scope="col">
              <span className="sr-only">Actions</span>
            </th>
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
                <td title={t.path}>{trackTitle(t)}<TrackMetadataDetails track={t} /></td>
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
