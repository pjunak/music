import { useCallback, useEffect, useState } from "react";
import type { FormEvent } from "react";

import { confirmDialog } from "@/components/ConfirmDialog";
import { api, libraryApi, modesApi, playlistsApi } from "@/core/api";
import { selectActiveTrackId, usePlayerStore } from "@/core/playerStore";
import { toast } from "@/core/toast";
import type { ModeSummary, PlaylistMeta, Track, TrackInPlaylist } from "@/core/types";
import { wsClient } from "@/core/ws";

export function PlaylistsView() {
  const [modes, setModes] = useState<ModeSummary[]>([]);
  const [filterMode, setFilterMode] = useState<string>("");
  const [playlists, setPlaylists] = useState<PlaylistMeta[]>([]);
  const [selected, setSelected] = useState<PlaylistMeta | null>(null);
  const [creating, setCreating] = useState(false);

  const refresh = useCallback(async () => {
    try {
      const list = await playlistsApi.list(
        filterMode ? { mode_id: filterMode } : {},
      );
      setPlaylists(list);
      // If our selected playlist disappeared, clear selection.
      if (selected !== null && !list.some((p) => p.id === selected.id)) {
        setSelected(null);
      }
    } catch (e) {
      toast.error("Load failed", e instanceof Error ? e.message : undefined);
    }
  }, [filterMode, selected]);

  useEffect(() => {
    void modesApi.list().then(setModes).catch(() => undefined);
  }, []);

  useEffect(() => {
    void refresh();
  }, [refresh]);

  return (
    <div className="playlists-view">
      <div className="playlists-pane playlists-list-pane">
        <header className="playlists-header">
          <h2>Playlists</h2>
          <button
            type="button"
            className="btn-primary"
            onClick={() => setCreating(true)}
          >
            + New
          </button>
        </header>
        <div className="playlists-filter">
          <label>
            <span className="muted small">Mode</span>
            <select
              value={filterMode}
              onChange={(e) => setFilterMode(e.target.value)}
            >
              <option value="">— all —</option>
              {modes.map((m) => (
                <option key={m.id} value={m.id}>
                  {m.name}
                </option>
              ))}
            </select>
          </label>
        </div>
        <ul className="playlist-list">
          {playlists.length === 0 ? (
            <li className="muted small empty">
              {filterMode === ""
                ? "No playlists yet. Click + New to make one."
                : "No playlists for this mode."}
            </li>
          ) : (
            playlists.map((p) => {
              const isSelected = selected?.id === p.id;
              return (
                <li
                  key={p.id}
                  className={`playlist-list-item ${isSelected ? "active" : ""}`}
                >
                  <button
                    type="button"
                    className="playlist-list-item-meta btn-ghost"
                    onClick={() => setSelected(p)}
                  >
                    <span className="playlist-name">{p.name}</span>
                    <span className="muted small">
                      {p.category ? `${p.category} · ` : ""}
                      {p.mode_id ? p.mode_id : "global"}
                    </span>
                  </button>
                  <button
                    type="button"
                    onClick={() =>
                      wsClient.send({
                        type: "ambient_play_playlist",
                        playlist_id: p.id,
                      })
                    }
                    title="Play this playlist now"
                  >
                    ▶
                  </button>
                </li>
              );
            })
          )}
        </ul>
      </div>

      <div className="playlists-pane playlists-detail-pane">
        {creating ? (
          <CreatePlaylistForm
            modes={modes}
            onClose={() => setCreating(false)}
            onCreated={async (p) => {
              setCreating(false);
              await refresh();
              setSelected(p);
            }}
          />
        ) : selected !== null ? (
          <PlaylistDetail
            playlist={selected}
            modes={modes}
            onChanged={refresh}
            onDeleted={() => {
              setSelected(null);
              void refresh();
            }}
          />
        ) : (
          <div className="empty-detail">
            <p className="muted">Select a playlist on the left, or click <strong>+ New</strong>.</p>
          </div>
        )}
      </div>
    </div>
  );
}

// --- create form -------------------------------------------------------

function CreatePlaylistForm({
  modes,
  onClose,
  onCreated,
}: {
  modes: ModeSummary[];
  onClose: () => void;
  onCreated: (p: PlaylistMeta) => void;
}) {
  const [name, setName] = useState("");
  const [modeId, setModeId] = useState<string>("");
  const [category, setCategory] = useState<string>("");
  const [busy, setBusy] = useState(false);

  async function submit(e: FormEvent) {
    e.preventDefault();
    setBusy(true);
    try {
      const p = await playlistsApi.create({
        name: name.trim(),
        mode_id: modeId || null,
        category: category.trim() || null,
      });
      toast.success("Playlist created", p.name);
      onCreated(p);
    } catch (err) {
      toast.error("Create failed", err instanceof Error ? err.message : undefined);
    } finally {
      setBusy(false);
    }
  }

  return (
    <form onSubmit={submit} className="metadata-form playlist-form">
      <h3>New playlist</h3>
      <label>
        <span>Name</span>
        <input
          required
          value={name}
          onChange={(e) => setName(e.target.value)}
          autoFocus
        />
      </label>
      <label>
        <span>Mode (optional)</span>
        <select value={modeId} onChange={(e) => setModeId(e.target.value)}>
          <option value="">global</option>
          {modes.map((m) => (
            <option key={m.id} value={m.id}>
              {m.name}
            </option>
          ))}
        </select>
      </label>
      <label>
        <span>Category (optional)</span>
        <input value={category} onChange={(e) => setCategory(e.target.value)} />
      </label>
      <div className="modal-actions">
        <button type="button" onClick={onClose} disabled={busy}>
          Cancel
        </button>
        <button type="submit" className="btn-primary" disabled={busy || !name.trim()}>
          {busy ? "Creating…" : "Create"}
        </button>
      </div>
    </form>
  );
}

// --- detail view -------------------------------------------------------

function PlaylistDetail({
  playlist,
  modes,
  onChanged,
  onDeleted,
}: {
  playlist: PlaylistMeta;
  modes: ModeSummary[];
  onChanged: () => Promise<void>;
  onDeleted: () => void;
}) {
  const activeTrackId = usePlayerStore(selectActiveTrackId);
  const [tracks, setTracks] = useState<TrackInPlaylist[]>([]);
  const [loading, setLoading] = useState(false);

  const refreshTracks = useCallback(async () => {
    setLoading(true);
    try {
      const r = await playlistsApi.tracks(playlist.id);
      setTracks(r);
    } catch (e) {
      toast.error("Load failed", e instanceof Error ? e.message : undefined);
    } finally {
      setLoading(false);
    }
  }, [playlist.id]);

  useEffect(() => {
    void refreshTracks();
  }, [refreshTracks]);

  async function removeAt(position: number) {
    try {
      await playlistsApi.removeTrack(playlist.id, position);
      await refreshTracks();
    } catch (e) {
      toast.error("Remove failed", e instanceof Error ? e.message : undefined);
    }
  }

  async function addTrack(track: Track) {
    try {
      await playlistsApi.addTrack(playlist.id, track.id);
      await refreshTracks();
      toast.success("Added", track.title || track.path);
    } catch (e) {
      toast.error("Add failed", e instanceof Error ? e.message : undefined);
    }
  }

  async function deletePlaylist() {
    const ok = await confirmDialog({
      title: `Delete "${playlist.name}"?`,
      body: "The tracks themselves stay in your library; only the list is removed.",
      tone: "danger",
      confirmLabel: "Delete",
    });
    if (!ok) return;
    try {
      await playlistsApi.delete(playlist.id);
      toast.success("Playlist deleted");
      onDeleted();
    } catch (e) {
      toast.error("Delete failed", e instanceof Error ? e.message : undefined);
    }
  }

  return (
    <div className="playlist-detail">
      <header className="playlist-detail-header">
        <div>
          <h2>{playlist.name}</h2>
          <p className="muted small">
            {playlist.category ? `${playlist.category} · ` : ""}
            {playlist.mode_id ? `mode ${playlist.mode_id}` : "global"}
          </p>
        </div>
        <div className="playlist-detail-actions">
          <button
            type="button"
            onClick={() =>
              wsClient.send({
                type: "ambient_play_playlist",
                playlist_id: playlist.id,
              })
            }
          >
            ▶ Play
          </button>
          <button type="button" className="btn-danger" onClick={() => void deletePlaylist()}>
            🗑 Delete
          </button>
        </div>
      </header>

      <PlaylistMetaEditor playlist={playlist} modes={modes} onSaved={onChanged} />

      <section>
        <h3>Tracks ({tracks.length})</h3>
        {loading ? (
          <p className="muted small">Loading…</p>
        ) : tracks.length === 0 ? (
          <p className="muted small">No tracks yet. Use the search below to add some.</p>
        ) : (
          <ol className="playlist-track-list">
            {tracks.map((row) => {
              const t = row.track;
              const isPlaying = activeTrackId === row.track_id;
              return (
                <li
                  key={`${row.position}-${row.track_id}`}
                  className={`playlist-track ${isPlaying ? "playing" : ""}`}
                >
                  <span className="playlist-track-pos muted small">{row.position + 1}</span>
                  <div className="playlist-track-meta">
                    <span className="playlist-track-title">
                      {t?.title || t?.path || `Track ${row.track_id}`}
                    </span>
                    {t?.artist ? <span className="muted small">{t.artist}</span> : null}
                  </div>
                  <div className="playlist-track-actions">
                    <button
                      onClick={() =>
                        wsClient.send({ type: "ambient_play_track", track_id: row.track_id })
                      }
                      title="Play"
                    >
                      ▶
                    </button>
                    <button
                      className="btn-danger"
                      onClick={() => void removeAt(row.position)}
                      title="Remove from playlist"
                    >
                      ✕
                    </button>
                  </div>
                </li>
              );
            })}
          </ol>
        )}
      </section>

      <section>
        <h3>Add tracks</h3>
        <TrackPicker onPick={addTrack} excludeIds={tracks.map((r) => r.track_id)} />
      </section>
    </div>
  );
}

function PlaylistMetaEditor({
  playlist,
  modes,
  onSaved,
}: {
  playlist: PlaylistMeta;
  modes: ModeSummary[];
  onSaved: () => Promise<void>;
}) {
  const [name, setName] = useState(playlist.name);
  const [modeId, setModeId] = useState(playlist.mode_id ?? "");
  const [category, setCategory] = useState(playlist.category ?? "");
  const [busy, setBusy] = useState(false);

  useEffect(() => {
    setName(playlist.name);
    setModeId(playlist.mode_id ?? "");
    setCategory(playlist.category ?? "");
  }, [playlist]);

  const dirty =
    name.trim() !== playlist.name ||
    (modeId || null) !== playlist.mode_id ||
    (category.trim() || null) !== playlist.category;

  async function save() {
    setBusy(true);
    try {
      await api.patch(`/api/playlists/${playlist.id}`, {
        name: name.trim(),
        mode_id: modeId || null,
        category: category.trim() || null,
      });
      toast.success("Saved");
      await onSaved();
    } catch (e) {
      toast.error("Save failed", e instanceof Error ? e.message : undefined);
    } finally {
      setBusy(false);
    }
  }

  return (
    <section className="playlist-meta-editor">
      <div className="playlist-meta-fields">
        <label>
          <span className="muted small">Name</span>
          <input value={name} onChange={(e) => setName(e.target.value)} />
        </label>
        <label>
          <span className="muted small">Mode</span>
          <select value={modeId} onChange={(e) => setModeId(e.target.value)}>
            <option value="">global</option>
            {modes.map((m) => (
              <option key={m.id} value={m.id}>
                {m.name}
              </option>
            ))}
          </select>
        </label>
        <label>
          <span className="muted small">Category</span>
          <input value={category} onChange={(e) => setCategory(e.target.value)} />
        </label>
      </div>
      <button
        type="button"
        className="btn-primary"
        disabled={!dirty || busy || !name.trim()}
        onClick={() => void save()}
      >
        {busy ? "Saving…" : "Save changes"}
      </button>
    </section>
  );
}

function TrackPicker({
  onPick,
  excludeIds,
}: {
  onPick: (t: Track) => void;
  excludeIds: number[];
}) {
  const [q, setQ] = useState("");
  const [results, setResults] = useState<Track[]>([]);
  const [searching, setSearching] = useState(false);

  useEffect(() => {
    if (q.trim() === "") {
      setResults([]);
      return;
    }
    let cancelled = false;
    setSearching(true);
    const t = window.setTimeout(() => {
      void libraryApi
        .search({ q, limit: 30, sort: "artist", order: "asc" })
        .then((r) => {
          if (!cancelled) setResults(r.tracks);
        })
        .catch(() => {
          if (!cancelled) setResults([]);
        })
        .finally(() => {
          if (!cancelled) setSearching(false);
        });
    }, 200);
    return () => {
      cancelled = true;
      window.clearTimeout(t);
    };
  }, [q]);

  const exclude = new Set(excludeIds);
  const filtered = results.filter((t) => !exclude.has(t.id));

  return (
    <div className="track-picker">
      <input
        type="search"
        value={q}
        onChange={(e) => setQ(e.target.value)}
        placeholder="Search the library…"
      />
      {q && filtered.length === 0 ? (
        <p className="muted small">{searching ? "Searching…" : "No matches."}</p>
      ) : null}
      <ul className="track-picker-list">
        {filtered.map((t) => (
          <li key={t.id}>
            <span className="track-picker-meta">
              <strong>{t.title || t.path}</strong>
              {t.artist ? <span className="muted small">{t.artist}</span> : null}
            </span>
            <button type="button" onClick={() => onPick(t)}>
              + Add
            </button>
          </li>
        ))}
      </ul>
    </div>
  );
}
