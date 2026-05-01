import { useCallback, useEffect, useState } from "react";
import type { FormEvent } from "react";

import { confirmDialog } from "@/components/ConfirmDialog";
import { EmptyState } from "@/components/EmptyState";
import { IconButton } from "@/components/IconButton";
import {
  ArrowDownIcon,
  ArrowUpIcon,
  PlayIcon,
  TrashIcon,
  XIcon,
} from "@/components/icons";
import { TrackBrowser } from "@/components/TrackBrowser";
import { api, libraryApi, modesApi, playlistsApi } from "@/core/api";
import { selectActiveTrackId, usePlayerStore } from "@/core/playerStore";
import { toast } from "@/core/toast";
import { trackTitle } from "@/core/trackDisplay";
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
                  <IconButton
                    label="Play this playlist now"
                    icon={<PlayIcon />}
                    onClick={() =>
                      wsClient.send({
                        type: "ambient_play_playlist",
                        playlist_id: p.id,
                      })
                    }
                  />
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
            <EmptyState title="No playlist selected">
              Pick one from the list, or click <strong>+ New</strong> to start
              a fresh playlist.
            </EmptyState>
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

  async function moveTo(position: number, toPosition: number) {
    try {
      await playlistsApi.moveTrack(playlist.id, position, toPosition);
      await refreshTracks();
    } catch (e) {
      toast.error("Reorder failed", e instanceof Error ? e.message : undefined);
    }
  }

  async function addTrack(track: Track) {
    return addTrackById(track.id, trackTitle(track));
  }

  // Internal — both the click-add path and the drop-target path land here.
  // Title is optional (drop payload may not carry it); when missing we
  // fetch the track to get a label for the toast.
  async function addTrackById(id: number, title?: string) {
    try {
      await playlistsApi.addTrack(playlist.id, id);
      await refreshTracks();
      const label = title ?? (await libraryApi.getTrack(id).then(trackTitle).catch(() => null));
      toast.success("Added", label ?? `Track ${id}`);
    } catch (e) {
      toast.error("Add failed", e instanceof Error ? e.message : undefined);
    }
  }

  function handleTrackDrop(e: React.DragEvent<HTMLElement>) {
    e.preventDefault();
    const raw = e.dataTransfer.getData("application/json");
    if (!raw) return;
    try {
      const payload = JSON.parse(raw) as {
        kind?: string;
        id?: number;
        title?: string;
      };
      if (payload.kind === "playlist-track" && typeof payload.id === "number") {
        // Skip if already in the playlist — the TrackBrowser hides these
        // already, but a stale drag from before the latest fetch could
        // still arrive.
        if (tracks.some((r) => r.track_id === payload.id)) return;
        void addTrackById(payload.id, payload.title);
      }
    } catch {
      /* malformed payload — ignore */
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
          <IconButton
            label="Play this playlist"
            icon={<PlayIcon />}
            onClick={() =>
              wsClient.send({
                type: "ambient_play_playlist",
                playlist_id: playlist.id,
              })
            }
          >
            Play
          </IconButton>
          <IconButton
            label="Delete this playlist"
            icon={<TrashIcon />}
            variant="danger"
            onClick={() => void deletePlaylist()}
          >
            Delete
          </IconButton>
        </div>
      </header>

      <PlaylistMetaEditor playlist={playlist} modes={modes} onSaved={onChanged} />

      <section
        className="playlist-tracks-section"
        onDragOver={(e) => {
          // Only react if the drag carries our payload — anything else
          // (browser-native file drag etc.) we ignore.
          if (!e.dataTransfer.types.includes("application/json")) return;
          e.preventDefault();
          e.dataTransfer.dropEffect = "copy";
          (e.currentTarget as HTMLElement).classList.add("playlist-tracks-droptarget");
        }}
        onDragLeave={(e) => {
          (e.currentTarget as HTMLElement).classList.remove("playlist-tracks-droptarget");
        }}
        onDrop={(e) => {
          (e.currentTarget as HTMLElement).classList.remove("playlist-tracks-droptarget");
          handleTrackDrop(e);
        }}
      >
        <h3>Tracks ({tracks.length})</h3>
        {loading ? (
          <p className="muted small">Loading…</p>
        ) : tracks.length === 0 ? (
          <p className="muted small">
            No tracks yet. Drag from the browser below or click <strong>+</strong>.
          </p>
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
                      {trackTitle(t) || `Track ${row.track_id}`}
                    </span>
                    {t?.artist ? <span className="muted small">{t.artist}</span> : null}
                  </div>
                  <div className="playlist-track-actions">
                    <IconButton
                      label="Move up"
                      icon={<ArrowUpIcon />}
                      onClick={() => void moveTo(row.position, row.position - 1)}
                      disabled={row.position === 0}
                    />
                    <IconButton
                      label="Move down"
                      icon={<ArrowDownIcon />}
                      onClick={() => void moveTo(row.position, row.position + 1)}
                      disabled={row.position === tracks.length - 1}
                    />
                    <IconButton
                      label="Play this track"
                      icon={<PlayIcon />}
                      onClick={() =>
                        wsClient.send({ type: "ambient_play_track", track_id: row.track_id })
                      }
                    />
                    <IconButton
                      label="Remove from playlist"
                      icon={<XIcon />}
                      variant="danger"
                      onClick={() => void removeAt(row.position)}
                    />
                  </div>
                </li>
              );
            })}
          </ol>
        )}
      </section>

      <section>
        <h3>Add tracks</h3>
        <p className="muted small">
          Click <strong>+</strong> on a row, or drag a track up into the list above.
        </p>
        <TrackBrowser
          onPickTrack={addTrack}
          dragPayload={(t) => ({
            kind: "playlist-track",
            id: t.id,
            title: trackTitle(t),
          })}
          excludeIds={tracks.map((r) => r.track_id)}
        />
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

