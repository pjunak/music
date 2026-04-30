import { useCallback, useEffect, useState } from "react";
import type { FormEvent } from "react";

import { confirmDialog } from "@/components/ConfirmDialog";
import { EmptyState } from "@/components/EmptyState";
import { IconButton } from "@/components/IconButton";
import { EditIcon, LightningIcon, TrashIcon } from "@/components/icons";
import { SceneEditor } from "@/components/SceneEditor";
import { SoundboardEditor } from "@/components/SoundboardEditor";
import { modesAdminApi, modesApi, playlistsApi } from "@/core/api";
import { toast } from "@/core/toast";
import type { InterruptSpec, ModeDetail, ModeSummary } from "@/core/types";
import { wsClient } from "@/core/ws";

export function ModesView() {
  const [modes, setModes] = useState<ModeSummary[]>([]);
  const [selectedId, setSelectedId] = useState<string | null>(null);
  const [creating, setCreating] = useState(false);
  // When the user clicks "Edit" on a soundboard or scene, we replace the
  // right pane with the corresponding editor. Cleared via "Back to mode".
  const [editingSoundboard, setEditingSoundboard] = useState<{
    modeId: string;
    soundboardId: string;
  } | null>(null);
  const [editingScene, setEditingScene] = useState<{
    modeId: string;
    sceneId: string;
  } | null>(null);

  const refresh = useCallback(async () => {
    try {
      const list = await modesApi.list();
      setModes(list);
      if (selectedId !== null && !list.some((m) => m.id === selectedId)) {
        setSelectedId(null);
        setEditingSoundboard(null);
      }
    } catch (e) {
      toast.error("Load failed", e instanceof Error ? e.message : undefined);
    }
  }, [selectedId]);

  // When the selected mode changes, drop any open sub-editor — its target
  // may not exist in the new mode.
  useEffect(() => {
    setEditingSoundboard(null);
    setEditingScene(null);
  }, [selectedId]);

  useEffect(() => {
    void refresh();
  }, [refresh]);

  return (
    <div className="modes-view">
      <div className="modes-pane modes-list-pane">
        <header className="playlists-header">
          <h2>Modes</h2>
          <button
            type="button"
            className="btn-primary"
            onClick={() => setCreating(true)}
          >
            + New
          </button>
        </header>
        <p className="muted small">
          Modes live as YAML on disk under <code>MODES_DIR</code>. The UI lets
          you scaffold a new one and add/remove its scenes & soundboards;
          deeper edits (scene contents, soundboard items) happen in the YAML
          for now.
        </p>
        <ul className="playlist-list">
          {modes.length === 0 ? (
            <li className="muted small empty">
              No modes loaded. Click + New to scaffold one.
            </li>
          ) : (
            modes.map((m) => {
              const isSelected = m.id === selectedId;
              return (
                <li
                  key={m.id}
                  className={`playlist-list-item ${isSelected ? "active" : ""}`}
                >
                  <button
                    type="button"
                    className="playlist-list-item-meta btn-ghost"
                    onClick={() => setSelectedId(m.id)}
                  >
                    <span className="playlist-name">{m.name}</span>
                    <span className="muted small">id: {m.id}</span>
                  </button>
                </li>
              );
            })
          )}
        </ul>
      </div>

      <div className="modes-pane modes-detail-pane">
        {creating ? (
          <CreateModeForm
            onClose={() => setCreating(false)}
            onCreated={async (id) => {
              setCreating(false);
              await refresh();
              setSelectedId(id);
            }}
          />
        ) : editingSoundboard !== null ? (
          <SoundboardEditor
            modeId={editingSoundboard.modeId}
            soundboardId={editingSoundboard.soundboardId}
            onBack={() => setEditingSoundboard(null)}
          />
        ) : editingScene !== null ? (
          <SceneEditor
            modeId={editingScene.modeId}
            sceneId={editingScene.sceneId}
            onBack={() => setEditingScene(null)}
          />
        ) : selectedId !== null ? (
          <ModeDetailPane
            modeId={selectedId}
            onChanged={refresh}
            onDeleted={() => {
              setSelectedId(null);
              void refresh();
            }}
            onEditSoundboard={(sbId) =>
              setEditingSoundboard({ modeId: selectedId, soundboardId: sbId })
            }
            onEditScene={(scId) =>
              setEditingScene({ modeId: selectedId, sceneId: scId })
            }
          />
        ) : (
          <div className="empty-detail">
            <EmptyState title="No mode selected">
              Pick one from the list, or click <strong>+ New</strong> to scaffold
              a fresh mode under <code>MODES_DIR</code>.
            </EmptyState>
          </div>
        )}
      </div>
    </div>
  );
}

function CreateModeForm({
  onClose,
  onCreated,
}: {
  onClose: () => void;
  onCreated: (id: string) => void;
}) {
  const [id, setId] = useState("");
  const [name, setName] = useState("");
  const [busy, setBusy] = useState(false);

  async function submit(e: FormEvent) {
    e.preventDefault();
    setBusy(true);
    try {
      await modesAdminApi.create(id.trim(), name.trim());
      toast.success("Mode created", id);
      onCreated(id);
    } catch (err) {
      toast.error("Create failed", err instanceof Error ? err.message : undefined);
    } finally {
      setBusy(false);
    }
  }

  return (
    <form onSubmit={submit} className="metadata-form playlist-form">
      <h3>New mode</h3>
      <label>
        <span>ID (slug)</span>
        <input
          required
          value={id}
          onChange={(e) => setId(e.target.value)}
          placeholder="dnd, cyberpunk, …"
          pattern="[a-z0-9][a-z0-9_-]*"
          title="lowercase letters/digits with optional dashes/underscores, starting with a letter or digit"
          autoFocus
        />
        <small className="muted">
          Lower-case slug (becomes the directory name under <code>MODES_DIR</code>).
        </small>
      </label>
      <label>
        <span>Display name</span>
        <input
          required
          value={name}
          onChange={(e) => setName(e.target.value)}
          placeholder="Dungeons & Dragons"
        />
      </label>
      <div className="modal-actions">
        <button type="button" onClick={onClose} disabled={busy}>
          Cancel
        </button>
        <button
          type="submit"
          className="btn-primary"
          disabled={busy || !id.trim() || !name.trim()}
        >
          {busy ? "Creating…" : "Create"}
        </button>
      </div>
    </form>
  );
}

function ModeDetailPane({
  modeId,
  onChanged,
  onDeleted,
  onEditSoundboard,
  onEditScene,
}: {
  modeId: string;
  onChanged: () => Promise<void>;
  onDeleted: () => void;
  onEditSoundboard: (soundboardId: string) => void;
  onEditScene: (sceneId: string) => void;
}) {
  const [detail, setDetail] = useState<ModeDetail | null>(null);
  const [loading, setLoading] = useState(false);

  const load = useCallback(async () => {
    setLoading(true);
    try {
      const d = await modesApi.get(modeId);
      setDetail(d);
    } catch (e) {
      toast.error("Load failed", e instanceof Error ? e.message : undefined);
    } finally {
      setLoading(false);
    }
  }, [modeId]);

  useEffect(() => {
    void load();
  }, [load]);

  async function deleteMode() {
    const ok = await confirmDialog({
      title: `Delete mode "${modeId}"?`,
      body: "The whole mode directory (manifest, soundboards, scenes, theme) will be removed from disk.",
      tone: "danger",
      confirmLabel: "Delete",
    });
    if (!ok) return;
    try {
      await modesAdminApi.delete(modeId);
      toast.success("Mode deleted");
      onDeleted();
    } catch (e) {
      toast.error("Delete failed", e instanceof Error ? e.message : undefined);
    }
  }

  if (loading || detail === null) {
    return <p className="muted small">Loading…</p>;
  }

  return (
    <div className="mode-detail">
      <header className="playlist-detail-header">
        <div>
          <h2>{detail.name}</h2>
          <p className="muted small">
            id: {detail.id} · {detail.has_theme ? "themed" : "no theme"} · default
            crossfade {detail.default_crossfade_ms}ms
          </p>
        </div>
        <div className="playlist-detail-actions">
          <IconButton
            label="Delete mode"
            icon={<TrashIcon />}
            variant="danger"
            onClick={() => void deleteMode()}
          >
            Delete mode
          </IconButton>
        </div>
      </header>

      <SubresourceList
        kind="soundboard"
        title="Soundboards"
        items={Object.keys(detail.soundboards)}
        onCreate={(id, name) =>
          modesAdminApi.createSoundboard(modeId, name ? { id, name } : { id })
        }
        onDelete={(id) => modesAdminApi.deleteSoundboard(modeId, id)}
        onEdit={onEditSoundboard}
        onChanged={async () => {
          await load();
          await onChanged();
        }}
      />

      <SubresourceList
        kind="scene"
        title="Scenes"
        items={Object.keys(detail.scenes)}
        onCreate={(id, name) =>
          modesAdminApi.createScene(modeId, { id, name: name || id })
        }
        onDelete={(id) => modesAdminApi.deleteScene(modeId, id)}
        onEdit={onEditScene}
        onChanged={async () => {
          await load();
          await onChanged();
        }}
      />

      <InterruptTemplatesEditor
        modeId={modeId}
        detail={detail}
        onChanged={async () => {
          await load();
          await onChanged();
        }}
      />
    </div>
  );
}


function InterruptTemplatesEditor({
  modeId,
  detail,
  onChanged,
}: {
  modeId: string;
  detail: ModeDetail;
  onChanged: () => Promise<void>;
}) {
  const [adding, setAdding] = useState(false);
  const [editingIdx, setEditingIdx] = useState<number | null>(null);

  async function remove(idx: number) {
    const ok = await confirmDialog({
      title: `Delete interrupt "${detail.interrupts[idx]?.name ?? ""}"?`,
      tone: "danger",
      confirmLabel: "Delete",
    });
    if (!ok) return;
    try {
      await modesAdminApi.deleteInterrupt(modeId, idx);
      toast.success("Interrupt deleted");
      await onChanged();
    } catch (e) {
      toast.error("Delete failed", e instanceof Error ? e.message : undefined);
    }
  }

  async function fire(spec: InterruptSpec) {
    if (spec.playlist) {
      // Resolve the named playlist to an id, then fire it. We accept either
      // mode-scoped playlists or globals — same lookup as scene activation.
      try {
        const matches = await playlistsApi.list({ mode_id: modeId });
        const found =
          matches.find((p) => p.name === spec.playlist) ?? null;
        if (found === null) {
          toast.error(
            "Playlist not found",
            `No playlist named "${spec.playlist ?? ""}" in this mode.`,
          );
          return;
        }
        wsClient.send({
          type: "fire_interrupt_playlist",
          playlist_id: found.id,
          return_to_ambient: spec.return_to_ambient ?? true,
          fade_in_ms: spec.fade_in_ms ?? 0,
          fade_out_ms: spec.fade_out_ms ?? 0,
        });
        toast.info("Interrupt fired", spec.name);
      } catch (e) {
        toast.error("Fire failed", e instanceof Error ? e.message : undefined);
      }
      return;
    }
    if (spec.soundboard_item) {
      // Single SFX template — fire as a one-shot SFX through the active
      // soundboard. Requires the active soundboard to actually contain it.
      const soundboardId = detail.default_soundboard;
      if (soundboardId === null) {
        toast.error(
          "No default soundboard",
          "Set one in the mode manifest to fire SFX-style interrupts.",
        );
        return;
      }
      wsClient.send({
        type: "fire_sfx",
        soundboard_id: soundboardId,
        item_path: spec.soundboard_item,
      });
      toast.info("SFX fired", spec.name);
    }
  }

  return (
    <section className="subresource-list">
      <div className="subresource-header">
        <h3>Interrupt templates</h3>
        <button type="button" onClick={() => setAdding((v) => !v)}>
          {adding ? "Cancel" : "+ Add"}
        </button>
      </div>
      <p className="muted small">
        Pre-configured interrupts the operator can fire from this list.
        Reference either a playlist (by name) or a soundboard item (by path
        relative to <code>SFX_LIBRARY_DIR</code>).
      </p>

      {adding ? (
        <InterruptTemplateForm
          modeId={modeId}
          mode="create"
          onClose={() => setAdding(false)}
          onSaved={async () => {
            setAdding(false);
            await onChanged();
          }}
        />
      ) : null}

      {detail.interrupts.length === 0 && !adding ? (
        <p className="muted small">None yet. Click + Add to create one.</p>
      ) : (
        <ul className="simple-list">
          {detail.interrupts.map((it, idx) =>
            editingIdx === idx ? (
              <li key={idx}>
                <InterruptTemplateForm
                  modeId={modeId}
                  mode="edit"
                  index={idx}
                  initial={it}
                  onClose={() => setEditingIdx(null)}
                  onSaved={async () => {
                    setEditingIdx(null);
                    await onChanged();
                  }}
                />
              </li>
            ) : (
              <li key={idx}>
                <span>
                  <strong>{it.name}</strong>
                  {it.playlist ? <> · playlist <code>{it.playlist}</code></> : null}
                  {it.soundboard_item ? (
                    <> · sfx <code>{it.soundboard_item}</code></>
                  ) : null}
                  {it.fade_in_ms || it.fade_out_ms ? (
                    <span className="muted small">
                      {" "}
                      · fade {it.fade_in_ms ?? 0} / {it.fade_out_ms ?? 0} ms
                    </span>
                  ) : null}
                  {it.return_to_ambient === false ? (
                    <span className="muted small"> · stops on end</span>
                  ) : null}
                </span>
                <span className="simple-list-actions">
                  <IconButton
                    label="Fire now"
                    icon={<LightningIcon />}
                    onClick={() => void fire(it)}
                  >
                    Fire
                  </IconButton>
                  <IconButton
                    label="Edit interrupt template"
                    icon={<EditIcon />}
                    onClick={() => setEditingIdx(idx)}
                  >
                    Edit
                  </IconButton>
                  <IconButton
                    label="Delete interrupt template"
                    icon={<TrashIcon />}
                    variant="danger"
                    onClick={() => void remove(idx)}
                  />
                </span>
              </li>
            ),
          )}
        </ul>
      )}
    </section>
  );
}


function InterruptTemplateForm({
  modeId,
  mode,
  index,
  initial,
  onClose,
  onSaved,
}: {
  modeId: string;
  mode: "create" | "edit";
  index?: number;
  initial?: InterruptSpec;
  onClose: () => void;
  onSaved: () => Promise<void>;
}) {
  type Source = "playlist" | "soundboard_item";
  const [name, setName] = useState(initial?.name ?? "");
  const [source, setSource] = useState<Source>(
    initial?.soundboard_item ? "soundboard_item" : "playlist",
  );
  const [playlist, setPlaylist] = useState(initial?.playlist ?? "");
  const [soundboardItem, setSoundboardItem] = useState(
    initial?.soundboard_item ?? "",
  );
  const [fadeInMs, setFadeInMs] = useState(initial?.fade_in_ms ?? 0);
  const [fadeOutMs, setFadeOutMs] = useState(initial?.fade_out_ms ?? 0);
  const [returnToAmbient, setReturnToAmbient] = useState(
    initial?.return_to_ambient ?? true,
  );
  const [busy, setBusy] = useState(false);

  async function submit(e: FormEvent) {
    e.preventDefault();
    setBusy(true);
    try {
      if (mode === "create") {
        const payload: Parameters<typeof modesAdminApi.addInterrupt>[1] = {
          name: name.trim(),
          fade_in_ms: fadeInMs,
          fade_out_ms: fadeOutMs,
          return_to_ambient: returnToAmbient,
        };
        if (source === "playlist") payload.playlist = playlist.trim();
        else payload.soundboard_item = soundboardItem.trim();
        await modesAdminApi.addInterrupt(modeId, payload);
        toast.success("Interrupt added");
      } else if (index !== undefined) {
        // For an edit, send everything explicitly so the server-side merge
        // doesn't keep stale fields when the operator switched source type.
        const payload: Parameters<typeof modesAdminApi.updateInterrupt>[2] = {
          name: name.trim(),
          fade_in_ms: fadeInMs,
          fade_out_ms: fadeOutMs,
          return_to_ambient: returnToAmbient,
          playlist: source === "playlist" ? playlist.trim() : null,
          soundboard_item:
            source === "soundboard_item" ? soundboardItem.trim() : null,
        };
        await modesAdminApi.updateInterrupt(modeId, index, payload);
        toast.success("Interrupt saved");
      }
      await onSaved();
    } catch (err) {
      toast.error("Save failed", err instanceof Error ? err.message : undefined);
    } finally {
      setBusy(false);
    }
  }

  return (
    <form onSubmit={submit} className="metadata-form interrupt-form">
      <div className="playlist-meta-fields">
        <label>
          <span className="muted small">Name</span>
          <input
            value={name}
            onChange={(e) => setName(e.target.value)}
            required
            autoFocus
          />
        </label>
        <label>
          <span className="muted small">Source</span>
          <select
            value={source}
            onChange={(e) => setSource(e.target.value as Source)}
          >
            <option value="playlist">Playlist (by name)</option>
            <option value="soundboard_item">Soundboard item (file path)</option>
          </select>
        </label>
        {source === "playlist" ? (
          <label>
            <span className="muted small">Playlist name</span>
            <input
              value={playlist}
              onChange={(e) => setPlaylist(e.target.value)}
              placeholder="tavern-music"
              required
            />
          </label>
        ) : (
          <label>
            <span className="muted small">SFX file path</span>
            <input
              value={soundboardItem}
              onChange={(e) => setSoundboardItem(e.target.value)}
              placeholder="dnd/door.ogg"
              required
            />
          </label>
        )}
        <label>
          <span className="muted small">Fade in (ms)</span>
          <input
            type="number"
            min={0}
            max={10000}
            value={fadeInMs}
            onChange={(e) => setFadeInMs(parseInt(e.target.value, 10) || 0)}
          />
        </label>
        <label>
          <span className="muted small">Fade out (ms)</span>
          <input
            type="number"
            min={0}
            max={10000}
            value={fadeOutMs}
            onChange={(e) => setFadeOutMs(parseInt(e.target.value, 10) || 0)}
          />
        </label>
        <label className="checkbox-row">
          <input
            type="checkbox"
            checked={returnToAmbient}
            onChange={(e) => setReturnToAmbient(e.target.checked)}
          />
          <span>Resume ambient when this interrupt ends</span>
        </label>
      </div>
      <div className="modal-actions">
        <button type="button" onClick={onClose} disabled={busy}>
          Cancel
        </button>
        <button type="submit" className="btn-primary" disabled={busy}>
          {busy ? "Saving…" : mode === "create" ? "Add" : "Save"}
        </button>
      </div>
    </form>
  );
}

function SubresourceList({
  kind,
  title,
  items,
  onCreate,
  onDelete,
  onChanged,
  onEdit,
}: {
  kind: "soundboard" | "scene";
  title: string;
  items: string[];
  onCreate: (id: string, name?: string) => Promise<unknown>;
  onDelete: (id: string) => Promise<void>;
  onChanged: () => Promise<void>;
  /** When provided, each row gets an Edit button that calls this with the
   *  item id. Used for soundboards (where we have a full editor); scenes
   *  don't have one yet. */
  onEdit?: (id: string) => void;
}) {
  const [showForm, setShowForm] = useState(false);
  const [id, setId] = useState("");
  const [name, setName] = useState("");
  const [busy, setBusy] = useState(false);

  async function submit(e: FormEvent) {
    e.preventDefault();
    setBusy(true);
    try {
      await onCreate(id.trim(), name.trim() || undefined);
      toast.success(`${title.slice(0, -1)} added`, id);
      setShowForm(false);
      setId("");
      setName("");
      await onChanged();
    } catch (e) {
      toast.error("Create failed", e instanceof Error ? e.message : undefined);
    } finally {
      setBusy(false);
    }
  }

  async function remove(itemId: string) {
    const ok = await confirmDialog({
      title: `Delete ${kind} "${itemId}"?`,
      tone: "danger",
      confirmLabel: "Delete",
    });
    if (!ok) return;
    try {
      await onDelete(itemId);
      toast.success(`${title.slice(0, -1)} deleted`);
      await onChanged();
    } catch (e) {
      toast.error("Delete failed", e instanceof Error ? e.message : undefined);
    }
  }

  return (
    <section className="subresource-list">
      <div className="subresource-header">
        <h3>{title}</h3>
        <button type="button" onClick={() => setShowForm((v) => !v)}>
          {showForm ? "Cancel" : "+ Add"}
        </button>
      </div>
      {showForm ? (
        <form onSubmit={submit} className="subresource-form">
          <input
            value={id}
            onChange={(e) => setId(e.target.value)}
            placeholder={`${kind} id`}
            pattern="[a-z0-9][a-z0-9_-]*"
            required
          />
          <input
            value={name}
            onChange={(e) => setName(e.target.value)}
            placeholder="Display name (optional)"
          />
          <button type="submit" className="btn-primary" disabled={busy}>
            {busy ? "Adding…" : "Add"}
          </button>
        </form>
      ) : null}
      {items.length === 0 ? (
        <p className="muted small">None yet.</p>
      ) : (
        <ul className="simple-list">
          {items.map((itemId) => (
            <li key={itemId}>
              <span>{itemId}</span>
              <span className="simple-list-actions">
                {onEdit ? (
                  <IconButton
                    label="Edit"
                    icon={<EditIcon />}
                    onClick={() => onEdit(itemId)}
                  >
                    Edit
                  </IconButton>
                ) : null}
                <IconButton
                  label="Delete"
                  icon={<TrashIcon />}
                  variant="danger"
                  onClick={() => void remove(itemId)}
                />
              </span>
            </li>
          ))}
        </ul>
      )}
    </section>
  );
}
