import { useCallback, useEffect, useMemo, useState } from "react";
import type { ChangeEvent, FormEvent } from "react";

import { confirmDialog } from "@/components/confirmDialog";
import { FolderTree } from "@/components/FolderTree";
import type { TreeFolder } from "@/components/FolderTree";
import { IconButton } from "@/components/IconButton";
import { EditIcon } from "@/components/icons";
import { MetadataEditor } from "@/components/MetadataEditor";
import { libraryApi } from "@/core/api";
import type { LibrarySortKey, MetadataUpdate, SortOrder } from "@/core/api";
import { toast } from "@/core/toast";
import { trackTitle } from "@/core/trackDisplay";
import type { Track } from "@/core/types";

const PAGE_SIZE = 200;

/** Client-side sort for folder-mode results (the tree() endpoint returns
 *  the folder's tracks in whatever order the index walked them, so we sort
 *  here to match the search() endpoint's behaviour). */
function sortTracks(tracks: Track[], sort: LibrarySortKey, order: SortOrder): Track[] {
  const dir = order === "asc" ? 1 : -1;
  const get = (t: Track): string | number => {
    switch (sort) {
      case "artist":
        return (t.artist ?? "").toLowerCase();
      case "album":
        return (t.album ?? "").toLowerCase();
      case "title":
        return (t.title || trackTitle(t)).toLowerCase();
      case "path":
        return t.path.toLowerCase();
      case "year":
        return t.year ?? -Infinity;
      case "added_at":
        return t.added_at ?? "";
      default:
        return "";
    }
  };
  return [...tracks].sort((a, b) => {
    const av = get(a);
    const bv = get(b);
    if (av < bv) return -1 * dir;
    if (av > bv) return 1 * dir;
    return 0;
  });
}

/** Bulk metadata management. Standalone tab so the operator can sweep
 *  many tracks at once (set Origin = "Skyrim" on a folder's worth of files,
 *  retag artists, etc.) without juggling per-track modals.
 *
 *  Layout: search/filter strip on top; multi-select table; right-hand
 *  bulk-edit panel that posts to PATCH /api/library/tracks/bulk-metadata.
 *  Per-track ✎ button still opens the same modal as the Library tab. */
export function MetadataView() {
  // Two browse modes: folder (default) and search. Active query overrides
  // the folder selection — clearing the search drops back to the chosen
  // folder. The track list updates in step.
  const [folderPath, setFolderPath] = useState("");
  const [query, setQuery] = useState("");
  const [pendingQuery, setPendingQuery] = useState("");
  const [sort, setSort] = useState<LibrarySortKey>("artist");
  const [order, setOrder] = useState<SortOrder>("asc");
  const [tracks, setTracks] = useState<Track[]>([]);
  const [total, setTotal] = useState(0);
  const [loading, setLoading] = useState(false);
  const [selected, setSelected] = useState<Set<number>>(new Set());
  const [editing, setEditing] = useState<Track | null>(null);
  const [refreshKey, setRefreshKey] = useState(0);
  const [movePickerOpen, setMovePickerOpen] = useState(false);
  const [bulkBusy, setBulkBusy] = useState(false);

  const inSearchMode = query !== "";

  // Reload tracks when the inputs change. Search mode hits /api/library/search
  // (server-side sort, paginated). Folder mode hits /api/library/tree to get
  // the immediate tracks for that folder, sorted client-side.
  useEffect(() => {
    let cancelled = false;
    setLoading(true);
    const fetcher = inSearchMode
      ? libraryApi
          .search({ q: query, limit: PAGE_SIZE, sort, order })
          .then((r) => ({ tracks: r.tracks, total: r.total }))
      : libraryApi.tree(folderPath).then((r) => ({
          tracks: sortTracks(r.tracks, sort, order),
          total: r.tracks.length,
        }));
    void fetcher
      .then((r) => {
        if (cancelled) return;
        setTracks(r.tracks);
        setTotal(r.total);
        // Drop ids from selection that aren't in the new result set —
        // otherwise "selected" can drift away from what's visible.
        const visible = new Set(r.tracks.map((t) => t.id));
        setSelected((prev) => new Set([...prev].filter((id) => visible.has(id))));
      })
      .catch((e: unknown) => {
        if (!cancelled) {
          toast.error(
            inSearchMode ? "Search failed" : "Folder load failed",
            e instanceof Error ? e.message : undefined,
          );
          setTracks([]);
          setTotal(0);
        }
      })
      .finally(() => {
        if (!cancelled) setLoading(false);
      });
    return () => {
      cancelled = true;
    };
  }, [inSearchMode, query, sort, order, folderPath, refreshKey]);

  const loadFolderChildren = useCallback(
    async (p: string): Promise<TreeFolder[]> => {
      const r = await libraryApi.tree(p);
      return r.folders.map((f) => ({
        name: f.name,
        path: f.path,
        badge: f.track_count > 0 ? f.track_count : null,
      }));
    },
    [],
  );

  function selectFolder(path: string) {
    // Picking a folder cancels any active search — they're mutually exclusive
    // since a search is already a global filter.
    setQuery("");
    setPendingQuery("");
    setFolderPath(path);
  }

  function submitQuery(e: FormEvent) {
    e.preventDefault();
    setQuery(pendingQuery);
  }

  function toggleAll() {
    if (selected.size === tracks.length) {
      setSelected(new Set());
    } else {
      setSelected(new Set(tracks.map((t) => t.id)));
    }
  }

  function toggleOne(id: number) {
    const next = new Set(selected);
    if (next.has(id)) next.delete(id);
    else next.add(id);
    setSelected(next);
  }

  const selectedTracks = useMemo(
    () => tracks.filter((t) => selected.has(t.id)),
    [tracks, selected],
  );

  const refresh = useCallback(() => {
    setRefreshKey((k) => k + 1);
  }, []);

  const selectionIds = useMemo(() => [...selected], [selected]);

  function reportBulkSkips(skipped: { track_id: number; reason: string }[]) {
    if (skipped.length === 0) return;
    const sample = skipped
      .slice(0, 3)
      .map((s) => `#${s.track_id}: ${s.reason}`)
      .join("\n");
    const more = skipped.length > 3 ? `\n…and ${skipped.length - 3} more` : "";
    toast.warn("Some tracks were skipped", `${sample}${more}`);
  }

  async function bulkMove(destination: string) {
    if (selectionIds.length === 0) return;
    setBulkBusy(true);
    try {
      const r = await libraryApi.bulkMove(selectionIds, destination);
      toast.success(
        `Moved ${r.moved.length} of ${selectionIds.length} track${selectionIds.length === 1 ? "" : "s"}`,
      );
      reportBulkSkips(r.skipped);
      setSelected(new Set());
      setMovePickerOpen(false);
      refresh();
    } catch (e) {
      toast.error("Bulk move failed", e instanceof Error ? e.message : undefined);
    } finally {
      setBulkBusy(false);
    }
  }

  async function bulkDelete() {
    if (selectionIds.length === 0) return;
    const ok = await confirmDialog({
      title: "Delete selected tracks?",
      body:
        `Permanently delete ${selectionIds.length} track${selectionIds.length === 1 ? "" : "s"} ` +
        "from disk and the library? Playlist references will be removed too.",
      tone: "danger",
      confirmLabel: "Delete",
    });
    if (!ok) return;
    setBulkBusy(true);
    try {
      const r = await libraryApi.bulkDelete(selectionIds);
      toast.success(
        `Deleted ${r.deleted_ids.length} track${r.deleted_ids.length === 1 ? "" : "s"}`,
      );
      reportBulkSkips(r.skipped);
      setSelected(new Set());
      refresh();
    } catch (e) {
      toast.error("Bulk delete failed", e instanceof Error ? e.message : undefined);
    } finally {
      setBulkBusy(false);
    }
  }

  return (
    <div className="metadata-view">
      <header className="metadata-toolbar">
        <form onSubmit={submitQuery} className="metadata-search">
          <input
            type="search"
            value={pendingQuery}
            onChange={(e) => setPendingQuery(e.target.value)}
            placeholder="Search title / display / artist / album / origin / path"
          />
          <button type="submit">Search</button>
          {query !== "" ? (
            <button
              type="button"
              onClick={() => {
                setPendingQuery("");
                setQuery("");
              }}
            >
              Clear
            </button>
          ) : null}
        </form>
        <SortControls sort={sort} order={order} onChange={(s, o) => {
          setSort(s);
          setOrder(o);
        }} />
        <span className="muted small">
          {loading
            ? "Loading…"
            : inSearchMode
              ? `${tracks.length} shown · ${total} total · ${selected.size} selected`
              : `${tracks.length} in folder · ${selected.size} selected`}
        </span>
        <div className="metadata-bulk-actions">
          <button
            type="button"
            disabled={selected.size === 0 || bulkBusy}
            onClick={() => setMovePickerOpen(true)}
            title={
              selected.size === 0
                ? "Select tracks first"
                : `Move ${selected.size} track${selected.size === 1 ? "" : "s"} to another folder`
            }
          >
            Move…
          </button>
          <button
            type="button"
            className="btn-danger"
            disabled={selected.size === 0 || bulkBusy}
            onClick={() => void bulkDelete()}
            title={
              selected.size === 0
                ? "Select tracks first"
                : `Delete ${selected.size} track${selected.size === 1 ? "" : "s"}`
            }
          >
            Delete…
          </button>
        </div>
      </header>

      <div className="metadata-body">
        <aside className="metadata-tree">
          <h3 className="metadata-tree-heading">Folder</h3>
          <FolderTree
            rootLabel="All music"
            selectedPath={inSearchMode ? "" : folderPath}
            onSelect={selectFolder}
            loadChildren={loadFolderChildren}
          />
        </aside>
        <section className="metadata-main">
          <table className="metadata-table">
            <thead>
              <tr>
                <th className="col-check">
                  <input
                    type="checkbox"
                    checked={tracks.length > 0 && selected.size === tracks.length}
                    onChange={toggleAll}
                    aria-label="Select all visible"
                  />
                </th>
                <th>Display title</th>
                <th>Title (tag)</th>
                <th>Artist</th>
                <th>Album</th>
                <th>Origin</th>
                <th className="col-num">Year</th>
                <th className="col-actions" />
              </tr>
            </thead>
            <tbody>
              {tracks.map((t) => {
                const isChecked = selected.has(t.id);
                return (
                  <tr
                    key={t.id}
                    className={`metadata-row ${isChecked ? "selected" : ""}`}
                    onClick={(e) => {
                      // Click anywhere on the row toggles selection except
                      // the actions cell (where buttons handle their own).
                      const target = e.target as HTMLElement;
                      if (target.closest(".col-actions")) return;
                      if (target.tagName === "INPUT") return;
                      toggleOne(t.id);
                    }}
                  >
                    <td className="col-check">
                      <input
                        type="checkbox"
                        checked={isChecked}
                        onChange={() => toggleOne(t.id)}
                        aria-label={`Select ${trackTitle(t)}`}
                      />
                    </td>
                    <td title={t.path}>
                      {t.display_title || <span className="muted">—</span>}
                    </td>
                    <td>{t.title || <span className="muted">—</span>}</td>
                    <td>{t.artist || <span className="muted">—</span>}</td>
                    <td>{t.album || <span className="muted">—</span>}</td>
                    <td>{t.origin || <span className="muted">—</span>}</td>
                    <td className="col-num">
                      {t.year ?? <span className="muted">—</span>}
                    </td>
                    <td className="col-actions">
                      <IconButton
                        label="Edit this track"
                        icon={<EditIcon />}
                        onClick={() => setEditing(t)}
                      />
                    </td>
                  </tr>
                );
              })}
              {tracks.length === 0 && !loading ? (
                <tr>
                  <td colSpan={8} className="muted small metadata-empty">
                    {inSearchMode
                      ? "No tracks match. Try a different search."
                      : folderPath === ""
                        ? "No tracks at the music root. Pick a subfolder, or upload via the Library tab."
                        : "This folder has no tracks at its top level. Pick a subfolder."}
                  </td>
                </tr>
              ) : null}
            </tbody>
          </table>
        </section>

        <aside className="metadata-aside">
          <BulkEditPanel
            selectedTracks={selectedTracks}
            onApplied={() => {
              setSelected(new Set());
              refresh();
            }}
          />
        </aside>
      </div>

      {editing !== null ? (
        <MetadataEditor
          track={editing}
          onClose={() => setEditing(null)}
          onSaved={() => {
            setEditing(null);
            refresh();
          }}
        />
      ) : null}

      {movePickerOpen ? (
        <BulkMovePicker
          count={selected.size}
          onCancel={() => setMovePickerOpen(false)}
          onConfirm={(dest) => void bulkMove(dest)}
          busy={bulkBusy}
        />
      ) : null}
    </div>
  );
}

/** Modal that picks a destination folder for the bulk-move action. Reuses
 *  the same FolderTree used in the main view so the operator's mental model
 *  matches: same widget, same lazy loading, same root-vs-subfolder semantics. */
function BulkMovePicker({
  count,
  onCancel,
  onConfirm,
  busy,
}: {
  count: number;
  onCancel: () => void;
  onConfirm: (dest: string) => void;
  busy: boolean;
}) {
  const [dest, setDest] = useState("");
  const loadChildren = useCallback(
    async (p: string): Promise<TreeFolder[]> => {
      const r = await libraryApi.tree(p);
      return r.folders.map((f) => ({
        name: f.name,
        path: f.path,
        badge: f.track_count > 0 ? f.track_count : null,
      }));
    },
    [],
  );
  return (
    <div
      className="modal-backdrop"
      onClick={(e) => {
        if (e.target === e.currentTarget) onCancel();
      }}
    >
      <div className="modal" role="dialog" aria-label="Move tracks to folder">
        <h2>
          Move {count} track{count === 1 ? "" : "s"}
        </h2>
        <p className="muted small">
          Pick the destination folder under MUSIC_DIR. Files keep their
          original names; collisions are skipped per-track.
        </p>
        <div className="metadata-tree">
          <FolderTree
            rootLabel="(music root)"
            selectedPath={dest}
            onSelect={setDest}
            loadChildren={loadChildren}
          />
        </div>
        <div className="modal-actions">
          <button type="button" onClick={onCancel} disabled={busy}>
            Cancel
          </button>
          <button
            type="button"
            className="btn-primary"
            onClick={() => onConfirm(dest)}
            disabled={busy}
          >
            {busy
              ? "Moving…"
              : `Move to ${dest === "" ? "(root)" : dest}`}
          </button>
        </div>
      </div>
    </div>
  );
}

function SortControls({
  sort,
  order,
  onChange,
}: {
  sort: LibrarySortKey;
  order: SortOrder;
  onChange: (sort: LibrarySortKey, order: SortOrder) => void;
}) {
  return (
    <label className="metadata-sort">
      <span className="muted small">Sort</span>
      <select
        value={sort}
        onChange={(e) => onChange(e.target.value as LibrarySortKey, order)}
      >
        <option value="artist">Artist</option>
        <option value="album">Album</option>
        <option value="title">Title</option>
        <option value="path">Path</option>
        <option value="year">Year</option>
        <option value="added_at">Added</option>
      </select>
      <select
        value={order}
        onChange={(e) => onChange(sort, e.target.value as SortOrder)}
      >
        <option value="asc">↑</option>
        <option value="desc">↓</option>
      </select>
    </label>
  );
}

/** Form on the right that posts a single bulk update.
 *
 *  Each field has an "apply" checkbox. Only fields with the box checked
 *  are sent — empty input + checked = clear the field across the
 *  selection; unchecked = leave the field alone. This makes the difference
 *  between "set to empty" and "don't change" explicit, since both look
 *  the same in plain inputs. */
function BulkEditPanel({
  selectedTracks,
  onApplied,
}: {
  selectedTracks: Track[];
  onApplied: () => void;
}) {
  const [fields, setFields] = useState<BulkFields>(emptyFields);
  const [busy, setBusy] = useState(false);

  // Reset the form when the selection clears entirely.
  useEffect(() => {
    if (selectedTracks.length === 0) setFields(emptyFields);
  }, [selectedTracks.length]);

  function update(key: keyof BulkFields, value: BulkField) {
    setFields((f) => ({ ...f, [key]: value }));
  }

  const ids = selectedTracks.map((t) => t.id);
  const anyApply = (Object.values(fields) as BulkField[]).some((f) => f.apply);

  async function apply() {
    if (!anyApply) {
      toast.info("Tick at least one field to apply.");
      return;
    }
    if (ids.length === 0) {
      toast.info("Select at least one track first.");
      return;
    }
    const updates = buildUpdates(fields);
    if (Object.keys(updates).length === 0) {
      toast.info("Nothing to apply.");
      return;
    }
    setBusy(true);
    try {
      const result = await libraryApi.updateBulkMetadata({
        track_ids: ids,
        updates,
      });
      const updated = result.updated.length;
      const skipped = result.skipped.length;
      if (skipped === 0) {
        toast.success(`Updated ${updated} track${updated === 1 ? "" : "s"}`);
      } else {
        // Surface the first few reasons; the operator can dig further if
        // there are many — usually they're all the same cause anyway.
        const sample = result.skipped
          .slice(0, 3)
          .map((s) => `#${s.track_id}: ${s.reason}`)
          .join("\n");
        const more = skipped > 3 ? `\n…and ${skipped - 3} more` : "";
        toast.warn(
          `Updated ${updated}, skipped ${skipped}`,
          `${sample}${more}`,
        );
      }
      onApplied();
    } catch (e) {
      toast.error("Bulk update failed", e instanceof Error ? e.message : undefined);
    } finally {
      setBusy(false);
    }
  }

  return (
    <div className="bulk-edit">
      <h2>Bulk edit</h2>
      <p className="muted small">
        {ids.length === 0
          ? "Select tracks on the left to apply changes."
          : `Will apply to ${ids.length} track${ids.length === 1 ? "" : "s"}.`}
      </p>

      <BulkRow
        label="Display title"
        hint="Overrides the tag-derived title in lists. DB-only."
        field={fields.display_title}
        onChange={(f) => update("display_title", f)}
      />
      <BulkRow
        label="Origin"
        hint="Source/provenance — game, film, album. DB-only."
        field={fields.origin}
        onChange={(f) => update("origin", f)}
      />

      <hr />
      <p className="muted small">Below: written back to the file's tags.</p>

      <BulkRow
        label="Artist"
        field={fields.artist}
        onChange={(f) => update("artist", f)}
      />
      <BulkRow
        label="Album artist"
        field={fields.album_artist}
        onChange={(f) => update("album_artist", f)}
      />
      <BulkRow
        label="Album"
        field={fields.album}
        onChange={(f) => update("album", f)}
      />
      <BulkRow
        label="Genre"
        field={fields.genre}
        onChange={(f) => update("genre", f)}
      />
      <BulkRowNumeric
        label="Year"
        field={fields.year}
        onChange={(f) => update("year", f)}
      />

      <button
        type="button"
        className="btn-primary bulk-apply"
        disabled={busy || ids.length === 0 || !anyApply}
        onClick={() => void apply()}
      >
        {busy ? "Applying…" : "Apply to selection"}
      </button>
    </div>
  );
}

interface BulkField {
  apply: boolean;
  value: string;
}

interface BulkFields {
  display_title: BulkField;
  origin: BulkField;
  artist: BulkField;
  album_artist: BulkField;
  album: BulkField;
  genre: BulkField;
  year: BulkField;
}

const emptyFields: BulkFields = {
  display_title: { apply: false, value: "" },
  origin: { apply: false, value: "" },
  artist: { apply: false, value: "" },
  album_artist: { apply: false, value: "" },
  album: { apply: false, value: "" },
  genre: { apply: false, value: "" },
  year: { apply: false, value: "" },
};

function buildUpdates(fields: BulkFields): MetadataUpdate {
  const out: MetadataUpdate = {};
  if (fields.display_title.apply) out.display_title = fields.display_title.value;
  if (fields.origin.apply) out.origin = fields.origin.value;
  if (fields.artist.apply) out.artist = fields.artist.value;
  if (fields.album_artist.apply) out.album_artist = fields.album_artist.value;
  if (fields.album.apply) out.album = fields.album.value;
  if (fields.genre.apply) out.genre = fields.genre.value;
  if (fields.year.apply) {
    out.year = fields.year.value === "" ? null : Number(fields.year.value);
  }
  return out;
}

function BulkRow({
  label,
  hint,
  field,
  onChange,
}: {
  label: string;
  hint?: string;
  field: BulkField;
  onChange: (f: BulkField) => void;
}) {
  function setApply(e: ChangeEvent<HTMLInputElement>) {
    onChange({ ...field, apply: e.target.checked });
  }
  function setValue(e: ChangeEvent<HTMLInputElement>) {
    // Typing in the input auto-arms the apply checkbox so the user doesn't
    // have to click the box separately. Clearing the input doesn't
    // disarm — leaving an explicit "set to empty" workflow possible.
    onChange({ apply: true, value: e.target.value });
  }
  return (
    <label className={`bulk-row${field.apply ? " bulk-row-armed" : ""}`}>
      <input
        type="checkbox"
        checked={field.apply}
        onChange={setApply}
        aria-label={`Apply ${label}`}
      />
      <span className="bulk-row-label">{label}</span>
      <input type="text" value={field.value} onChange={setValue} />
      {hint ? <span className="muted small bulk-row-hint">{hint}</span> : null}
    </label>
  );
}

function BulkRowNumeric({
  label,
  field,
  onChange,
}: {
  label: string;
  field: BulkField;
  onChange: (f: BulkField) => void;
}) {
  function setApply(e: ChangeEvent<HTMLInputElement>) {
    onChange({ ...field, apply: e.target.checked });
  }
  function setValue(e: ChangeEvent<HTMLInputElement>) {
    onChange({ apply: true, value: e.target.value });
  }
  return (
    <label className={`bulk-row${field.apply ? " bulk-row-armed" : ""}`}>
      <input
        type="checkbox"
        checked={field.apply}
        onChange={setApply}
        aria-label={`Apply ${label}`}
      />
      <span className="bulk-row-label">{label}</span>
      <input type="number" min={0} value={field.value} onChange={setValue} />
    </label>
  );
}
