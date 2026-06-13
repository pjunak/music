import { useCallback, useEffect, useState } from "react";
import type { DragEvent, FormEvent } from "react";

import { libraryApi } from "@/core/api";
import { toast } from "@/core/toast";
import { trackTitle } from "@/core/trackDisplay";
import type { Track } from "@/core/types";

import { FolderTree } from "./FolderTree";
import type { TreeFolder } from "./FolderTree";
import { IconButton } from "./IconButton";
import { PlusIcon } from "./icons";

/** Folder-tree + track-list browser, used wherever a part of the UI
 *  needs to pick or drag tracks from the music library — currently the
 *  playlist track-add flow, but designed to slot into the Library tab
 *  and other future pickers without reimplementing the tree + fetch
 *  logic each time.
 *
 *  Two interaction modes the host can opt into independently:
 *   - `onPickTrack` — clicking the row's "Add" button fires this. Use
 *     it for a click-to-add flow.
 *   - `dragPayload` — when set, each row is `draggable` and the JSON
 *     payload returned here lands in the dataTransfer. The host's drop
 *     target reads it back via `application/json`.
 *
 *  Search and folder navigation are mutually exclusive (selecting a
 *  folder clears the query, submitting a query clears the folder
 *  selection) — same convention the Metadata tab uses. */

const PAGE_SIZE = 200;

interface Props {
  /** Click handler for the row's "Add" button. When unset, no Add
   *  button is rendered and drag is the only way to pick a track. */
  onPickTrack?: (track: Track) => void;
  /** Drag-payload generator. When provided, rows become `draggable`
   *  and the payload (serialised as JSON) goes onto the dataTransfer
   *  under the `application/json` mime type. */
  dragPayload?: (track: Track) => unknown;
  /** Track ids to hide from the result list (e.g. tracks already in
   *  the playlist being edited). */
  excludeIds?: number[];
  /** Empty-state hint shown when the current view yields no tracks. */
  emptyHint?: string;
}

export function TrackBrowser({
  onPickTrack,
  dragPayload,
  excludeIds,
  emptyHint,
}: Props) {
  const [folderPath, setFolderPath] = useState("");
  const [query, setQuery] = useState("");
  const [pendingQuery, setPendingQuery] = useState("");
  const [tracks, setTracks] = useState<Track[]>([]);
  const [loading, setLoading] = useState(false);

  const inSearchMode = query !== "";
  const exclude = new Set(excludeIds ?? []);

  useEffect(() => {
    let cancelled = false;
    setLoading(true);
    const fetcher = inSearchMode
      ? libraryApi
          .search({ q: query, limit: PAGE_SIZE })
          .then((r) => r.tracks)
      : libraryApi.tree(folderPath).then((r) => r.tracks);
    void fetcher
      .then((all) => {
        if (cancelled) return;
        setTracks(all);
      })
      .catch((e: unknown) => {
        if (!cancelled) {
          toast.error(
            inSearchMode ? "Search failed" : "Folder load failed",
            e instanceof Error ? e.message : undefined,
          );
          setTracks([]);
        }
      })
      .finally(() => {
        if (!cancelled) setLoading(false);
      });
    return () => {
      cancelled = true;
    };
  }, [inSearchMode, query, folderPath]);

  const loadAllFolders = useCallback(async (): Promise<TreeFolder[]> => {
    const r = await libraryApi.allFolders();
    return r.folders.map((f) => ({
      name: f.name,
      path: f.path,
      badge: f.track_count > 0 ? f.track_count : null,
    }));
  }, []);

  function selectFolder(path: string) {
    setQuery("");
    setPendingQuery("");
    setFolderPath(path);
  }

  function submitQuery(e: FormEvent) {
    e.preventDefault();
    setQuery(pendingQuery);
  }

  function onTrackDragStart(e: DragEvent<HTMLLIElement>, track: Track) {
    if (!dragPayload) return;
    const payload = dragPayload(track);
    e.dataTransfer.setData("application/json", JSON.stringify(payload));
    e.dataTransfer.effectAllowed = "copy";
  }

  const visible = tracks.filter((t) => !exclude.has(t.id));

  return (
    <div className="track-browser">
      <form className="track-browser-search" onSubmit={submitQuery}>
        <input
          type="search"
          value={pendingQuery}
          onChange={(e) => setPendingQuery(e.target.value)}
          placeholder="Search title / artist / album / origin / path"
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
      <div className="track-browser-body">
        <aside className="track-browser-tree">
          <FolderTree
            selectedPath={inSearchMode ? "" : folderPath}
            onSelect={selectFolder}
            loadAll={loadAllFolders}
          />
        </aside>
        <ul className="track-browser-list">
          {loading ? (
            <li className="muted small track-browser-empty">Loading…</li>
          ) : visible.length === 0 ? (
            <li className="muted small track-browser-empty">
              {emptyHint ?? "No tracks here."}
            </li>
          ) : (
            visible.map((t) => (
              <li
                key={t.id}
                className="track-browser-row"
                draggable={dragPayload !== undefined}
                onDragStart={(e) => onTrackDragStart(e, t)}
                title={t.path}
              >
                <div className="track-browser-meta">
                  <span className="track-browser-title">
                    {trackTitle(t) || `Track ${t.id}`}
                  </span>
                  {t.artist ? (
                    <span className="muted small">{t.artist}</span>
                  ) : null}
                </div>
                {onPickTrack !== undefined ? (
                  <IconButton
                    label="Add to playlist"
                    icon={<PlusIcon />}
                    onClick={() => onPickTrack(t)}
                  />
                ) : null}
              </li>
            ))
          )}
        </ul>
      </div>
    </div>
  );
}
