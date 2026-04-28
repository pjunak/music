import { useCallback, useEffect, useMemo, useState } from "react";
import type { FormEvent } from "react";

import { libraryApi } from "@/core/api";
import type { LibrarySortKey, SearchResponse, SortOrder } from "@/core/api";
import { usePlayerStore } from "@/core/playerStore";
import type { Track } from "@/core/types";
import { wsClient } from "@/core/ws";
import { UploadManager } from "@/panels/UploadManager";

const PAGE_SIZE = 100;

interface ColumnDef {
  key: LibrarySortKey;
  label: string;
  /** Render value for a track. Null/undefined renders as a muted dash. */
  render: (t: Track) => string | number | null | undefined;
  className?: string;
}

const COLUMNS: ColumnDef[] = [
  { key: "title", label: "Title", render: (t) => t.title || "(untitled)" },
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

export function LibraryPanel() {
  const [query, setQuery] = useState("");
  const [pendingQuery, setPendingQuery] = useState("");
  const [sort, setSort] = useState<LibrarySortKey>("artist");
  const [order, setOrder] = useState<SortOrder>("asc");
  const [offset, setOffset] = useState(0);
  const [response, setResponse] = useState<SearchResponse | null>(null);
  const [loading, setLoading] = useState(false);
  const [error, setError] = useState<string | null>(null);

  const ambientCurrentId = usePlayerStore(
    (s) => s.state?.ambient.current_beets_id ?? null,
  );

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
        setResponse(res);
      } catch (e) {
        setError(e instanceof Error ? e.message : "search failed");
        setResponse(null);
      } finally {
        setLoading(false);
      }
    },
    [],
  );

  // Trigger a search whenever any of the controlled inputs change.
  useEffect(() => {
    void runSearch(query, { sort, order, offset });
  }, [runSearch, query, sort, order, offset]);

  function onSubmitSearch(e: FormEvent) {
    e.preventDefault();
    setOffset(0);
    setQuery(pendingQuery);
  }

  function onClickSort(key: LibrarySortKey) {
    if (sort === key) {
      setOrder(order === "asc" ? "desc" : "asc");
    } else {
      setSort(key);
      setOrder("asc");
    }
    setOffset(0);
  }

  function play(track: Track) {
    wsClient.send({ type: "ambient_play_track", beets_id: track.beets_id });
  }

  function enqueue(track: Track) {
    wsClient.send({ type: "ambient_enqueue", beets_id: track.beets_id });
  }

  function refineByText(text: string) {
    setPendingQuery(text);
    setQuery(text);
    setOffset(0);
  }

  const tracks = response?.tracks ?? [];
  const total = response?.total ?? 0;
  const showingFrom = total === 0 ? 0 : offset + 1;
  const showingTo = Math.min(offset + tracks.length, total);
  const canPrev = offset > 0;
  const canNext = offset + tracks.length < total;

  const headerCells = useMemo(
    () =>
      COLUMNS.map((col) => {
        const active = sort === col.key;
        const indicator = active ? (order === "asc" ? "▲" : "▼") : "";
        return (
          <th
            key={col.key}
            className={`sortable ${col.className ?? ""} ${active ? "sort-active" : ""}`}
            onClick={() => onClickSort(col.key)}
            scope="col"
          >
            <span>{col.label}</span>
            <span className="sort-indicator">{indicator}</span>
          </th>
        );
      }),
    // eslint-disable-next-line react-hooks/exhaustive-deps
    [sort, order],
  );

  return (
    <section className="panel library-panel">
      <h2>Library</h2>
      <UploadManager
        onIngestComplete={() => void runSearch(query, { sort, order, offset })}
      />
      <form onSubmit={onSubmitSearch} className="library-search">
        <input
          type="search"
          value={pendingQuery}
          onChange={(e) => setPendingQuery(e.target.value)}
          placeholder='Beets query — e.g. artist:daft year:2001..'
        />
        <button type="submit" disabled={loading}>
          {loading ? "…" : "Search"}
        </button>
        {query !== "" ? (
          <button
            type="button"
            onClick={() => {
              setPendingQuery("");
              setQuery("");
              setOffset(0);
            }}
            title="Clear query"
          >
            Clear
          </button>
        ) : null}
      </form>

      {error !== null ? <p className="error small">{error}</p> : null}

      <div className="library-meta">
        {loading ? (
          <span className="muted small">Loading…</span>
        ) : (
          <span className="muted small">
            {total === 0
              ? "No tracks match."
              : `${showingFrom}–${showingTo} of ${total}`}
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

      {tracks.length > 0 ? (
        <div className="track-table-wrap">
          <table className="track-table">
            <thead>
              <tr>
                {headerCells}
                <th className="col-actions" />
              </tr>
            </thead>
            <tbody>
              {tracks.map((t) => {
                const isPlaying = ambientCurrentId === t.beets_id;
                return (
                  <tr
                    key={t.beets_id}
                    className={isPlaying ? "track-row playing" : "track-row"}
                    onDoubleClick={() => play(t)}
                  >
                    {COLUMNS.map((col) => {
                      const v = col.render(t);
                      const display = v === null || v === undefined || v === "" ? "—" : v;
                      const isLink = col.key === "artist" || col.key === "album";
                      const isMuted = display === "—";
                      return (
                        <td
                          key={col.key}
                          className={`${col.className ?? ""} ${isMuted ? "muted" : ""}`}
                        >
                          {isLink && !isMuted ? (
                            <button
                              type="button"
                              className="cell-link"
                              onClick={() =>
                                refineByText(`${col.key}:"${String(v)}"`)
                              }
                              title={`Filter by ${col.label}`}
                            >
                              {display}
                            </button>
                          ) : (
                            display
                          )}
                        </td>
                      );
                    })}
                    <td className="col-actions">
                      <button type="button" onClick={() => play(t)} title="Play now">
                        ▶
                      </button>
                      <button
                        type="button"
                        onClick={() => enqueue(t)}
                        title="Add to queue"
                      >
                        ＋
                      </button>
                    </td>
                  </tr>
                );
              })}
            </tbody>
          </table>
        </div>
      ) : !loading ? (
        <p className="muted small">
          {total === 0 && query === ""
            ? "No tracks. Drop audio files into the Add music section above to get started."
            : null}
        </p>
      ) : null}
    </section>
  );
}
