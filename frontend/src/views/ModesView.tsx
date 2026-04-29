import { useCallback, useEffect, useState } from "react";
import type { FormEvent } from "react";

import { confirmDialog } from "@/components/ConfirmDialog";
import { SoundboardEditor } from "@/components/SoundboardEditor";
import { modesAdminApi, modesApi } from "@/core/api";
import { toast } from "@/core/toast";
import type { ModeDetail, ModeSummary } from "@/core/types";

export function ModesView() {
  const [modes, setModes] = useState<ModeSummary[]>([]);
  const [selectedId, setSelectedId] = useState<string | null>(null);
  const [creating, setCreating] = useState(false);
  // When the user clicks "Edit" on a soundboard, we replace the right
  // pane with the soundboard editor. Cleared via "Back to mode".
  const [editingSoundboard, setEditingSoundboard] = useState<{
    modeId: string;
    soundboardId: string;
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
          />
        ) : (
          <div className="empty-detail">
            <p className="muted">Select a mode on the left, or click <strong>+ New</strong>.</p>
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
}: {
  modeId: string;
  onChanged: () => Promise<void>;
  onDeleted: () => void;
  onEditSoundboard: (soundboardId: string) => void;
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
          <button type="button" className="btn-danger" onClick={() => void deleteMode()}>
            🗑 Delete mode
          </button>
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
        onChanged={async () => {
          await load();
          await onChanged();
        }}
      />

      <section>
        <h3>Interrupt templates</h3>
        {detail.interrupts.length === 0 ? (
          <p className="muted small">
            None defined. Edit the mode's <code>manifest.yaml</code> to add some
            (UI for this is planned in <code>docs/FUTURE.md</code>).
          </p>
        ) : (
          <ul className="simple-list">
            {detail.interrupts.map((it, idx) => (
              <li key={idx}>
                <strong>{it.name}</strong>
                {it.playlist ? <> · playlist <code>{it.playlist}</code></> : null}
                {it.soundboard_item ? (
                  <> · sfx <code>{it.soundboard_item}</code></>
                ) : null}
              </li>
            ))}
          </ul>
        )}
      </section>
    </div>
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
                  <button type="button" onClick={() => onEdit(itemId)}>
                    ✎ Edit
                  </button>
                ) : null}
                <button
                  type="button"
                  className="btn-danger"
                  onClick={() => void remove(itemId)}
                >
                  🗑
                </button>
              </span>
            </li>
          ))}
        </ul>
      )}
    </section>
  );
}
