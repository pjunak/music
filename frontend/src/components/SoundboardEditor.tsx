import { useCallback, useEffect, useState } from "react";
import type { FormEvent } from "react";

import { confirmDialog } from "@/components/ConfirmDialog";
import { modesAdminApi, modesApi, sfxApi } from "@/core/api";
import type { SfxFile } from "@/core/api";
import { toast } from "@/core/toast";
import type { SoundboardManifest } from "@/core/types";

interface Props {
  modeId: string;
  soundboardId: string;
  onBack: () => void;
}

/** Edit a single soundboard's categories and items. Loads the latest copy
 *  from `modesApi.get(modeId)`, applies edits via the admin endpoints, and
 *  re-renders from the response of each mutation. */
export function SoundboardEditor({ modeId, soundboardId, onBack }: Props) {
  const [soundboard, setSoundboard] = useState<SoundboardManifest | null>(null);
  const [sfxFiles, setSfxFiles] = useState<SfxFile[]>([]);
  const [error, setError] = useState<string | null>(null);

  const refresh = useCallback(async () => {
    try {
      const detail = await modesApi.get(modeId);
      const sb = detail.soundboards[soundboardId] ?? null;
      setSoundboard(sb);
      if (sb === null) {
        setError(`Soundboard "${soundboardId}" not found.`);
      } else {
        setError(null);
      }
    } catch (e) {
      setError(e instanceof Error ? e.message : "load failed");
    }
  }, [modeId, soundboardId]);

  useEffect(() => {
    void refresh();
  }, [refresh]);

  useEffect(() => {
    void sfxApi
      .allFiles()
      .then(setSfxFiles)
      .catch(() => setSfxFiles([]));
  }, []);

  async function addCategory() {
    const id = window.prompt("Category id (slug, e.g. doors):", "");
    if (!id) return;
    const name = window.prompt("Category display name:", id);
    if (!name) return;
    try {
      const updated = await modesAdminApi.addCategory(modeId, soundboardId, {
        id: id.trim(),
        name: name.trim(),
      });
      setSoundboard(updated as SoundboardManifest);
      toast.success("Category added", id);
    } catch (e) {
      toast.error("Add failed", e instanceof Error ? e.message : undefined);
    }
  }

  async function removeCategory(categoryId: string) {
    const ok = await confirmDialog({
      title: `Remove category "${categoryId}"?`,
      body: "All items in this category will be removed too. The underlying SFX files stay on disk.",
      tone: "danger",
      confirmLabel: "Remove",
    });
    if (!ok) return;
    try {
      await modesAdminApi.deleteCategory(modeId, soundboardId, categoryId);
      await refresh();
      toast.success("Category removed");
    } catch (e) {
      toast.error("Remove failed", e instanceof Error ? e.message : undefined);
    }
  }

  async function addItem(categoryId: string, payload: AddItemPayload) {
    try {
      const updated = await modesAdminApi.addItem(
        modeId,
        soundboardId,
        categoryId,
        payload,
      );
      setSoundboard(updated as SoundboardManifest);
      toast.success("SFX added", payload.name);
    } catch (e) {
      toast.error("Add failed", e instanceof Error ? e.message : undefined);
    }
  }

  async function updateItem(
    categoryId: string,
    index: number,
    payload: { name?: string; hotkey?: string },
  ) {
    try {
      const updated = await modesAdminApi.updateItem(
        modeId,
        soundboardId,
        categoryId,
        index,
        payload,
      );
      setSoundboard(updated as SoundboardManifest);
    } catch (e) {
      toast.error("Save failed", e instanceof Error ? e.message : undefined);
    }
  }

  async function deleteItem(categoryId: string, index: number) {
    try {
      const updated = await modesAdminApi.deleteItem(
        modeId,
        soundboardId,
        categoryId,
        index,
      );
      setSoundboard(updated as SoundboardManifest);
      toast.success("SFX removed");
    } catch (e) {
      toast.error("Remove failed", e instanceof Error ? e.message : undefined);
    }
  }

  if (error !== null) {
    return (
      <div className="soundboard-editor">
        <button type="button" className="btn-ghost back-link" onClick={onBack}>
          ← Back to mode
        </button>
        <p className="error small">{error}</p>
      </div>
    );
  }

  if (soundboard === null) {
    return <p className="muted small">Loading…</p>;
  }

  return (
    <div className="soundboard-editor">
      <header className="playlist-detail-header">
        <div>
          <button
            type="button"
            className="btn-ghost back-link"
            onClick={onBack}
          >
            ← Back to mode
          </button>
          <h2>Soundboard: {soundboard.name ?? soundboard.id}</h2>
          <p className="muted small">
            id: <code>{soundboard.id}</code> · mode <code>{modeId}</code>
          </p>
        </div>
        <div className="playlist-detail-actions">
          <button type="button" onClick={() => void addCategory()}>
            + Category
          </button>
        </div>
      </header>

      {soundboard.categories.length === 0 ? (
        <p className="muted small">
          No categories yet. Click <strong>+ Category</strong> above (e.g.
          "doors", "ambience") to start grouping SFX.
        </p>
      ) : (
        <ul className="soundboard-categories">
          {soundboard.categories.map((cat) => (
            <li key={cat.id} className="soundboard-category">
              <header>
                <div>
                  <h3>{cat.name}</h3>
                  <span className="muted small">id: {cat.id}</span>
                </div>
                <button
                  type="button"
                  className="btn-danger"
                  onClick={() => void removeCategory(cat.id)}
                  title="Remove category and all its items"
                >
                  🗑 Category
                </button>
              </header>

              <ItemList
                items={cat.items}
                onUpdate={(idx, p) => updateItem(cat.id, idx, p)}
                onDelete={(idx) => deleteItem(cat.id, idx)}
              />

              <AddItemForm
                sfxFiles={sfxFiles}
                onAdd={(payload) => addItem(cat.id, payload)}
              />
            </li>
          ))}
        </ul>
      )}
    </div>
  );
}

interface AddItemPayload {
  file: string;
  name: string;
  hotkey?: string;
}

function ItemList({
  items,
  onUpdate,
  onDelete,
}: {
  items: SoundboardManifest["categories"][number]["items"];
  onUpdate: (
    idx: number,
    payload: { name?: string; hotkey?: string },
  ) => Promise<void>;
  onDelete: (idx: number) => Promise<void>;
}) {
  if (items.length === 0) {
    return <p className="muted small">No SFX in this category yet.</p>;
  }
  return (
    <ol className="soundboard-items">
      {items.map((item, idx) => (
        <ItemRow
          key={`${item.file}-${idx}`}
          item={item}
          onSave={(payload) => onUpdate(idx, payload)}
          onDelete={() => onDelete(idx)}
        />
      ))}
    </ol>
  );
}

function ItemRow({
  item,
  onSave,
  onDelete,
}: {
  item: SoundboardManifest["categories"][number]["items"][number];
  onSave: (payload: { name?: string; hotkey?: string }) => Promise<void>;
  onDelete: () => Promise<void>;
}) {
  const [name, setName] = useState(item.name);
  const [hotkey, setHotkey] = useState(item.hotkey ?? "");

  useEffect(() => {
    setName(item.name);
    setHotkey(item.hotkey ?? "");
  }, [item.name, item.hotkey]);

  const dirty = name !== item.name || (hotkey || null) !== (item.hotkey ?? null);

  async function save() {
    const payload: { name?: string; hotkey?: string } = { name };
    payload.hotkey = hotkey;
    await onSave(payload);
  }

  return (
    <li className="soundboard-item">
      <code className="soundboard-item-file" title={item.file}>
        {item.file}
      </code>
      <input
        className="soundboard-item-name"
        value={name}
        onChange={(e) => setName(e.target.value)}
        placeholder="Display name"
      />
      <input
        className="soundboard-item-hotkey"
        value={hotkey}
        onChange={(e) => setHotkey(e.target.value.slice(0, 1))}
        maxLength={1}
        placeholder="key"
        title="Single-character keyboard hotkey, fired from anywhere when this soundboard is active"
      />
      <div className="soundboard-item-actions">
        <button
          type="button"
          className={dirty ? "btn-primary" : ""}
          onClick={() => void save()}
          disabled={!dirty}
        >
          Save
        </button>
        <button type="button" className="btn-danger" onClick={() => void onDelete()}>
          ✕
        </button>
      </div>
    </li>
  );
}

function AddItemForm({
  sfxFiles,
  onAdd,
}: {
  sfxFiles: SfxFile[];
  onAdd: (payload: AddItemPayload) => Promise<void>;
}) {
  const [file, setFile] = useState("");
  const [name, setName] = useState("");
  const [hotkey, setHotkey] = useState("");
  const [busy, setBusy] = useState(false);

  async function submit(e: FormEvent) {
    e.preventDefault();
    if (!file || !name) return;
    setBusy(true);
    try {
      const payload: AddItemPayload = { file, name };
      if (hotkey) payload.hotkey = hotkey;
      await onAdd(payload);
      setFile("");
      setName("");
      setHotkey("");
    } finally {
      setBusy(false);
    }
  }

  return (
    <form className="soundboard-add-item" onSubmit={submit}>
      <select
        value={file}
        onChange={(e) => {
          const v = e.target.value;
          setFile(v);
          // Suggest a name based on the filename if the field is still empty.
          if (!name && v) {
            const stem = v.split("/").pop()?.replace(/\.[^.]+$/, "") ?? "";
            setName(stem.replace(/[-_]+/g, " "));
          }
        }}
        required
      >
        <option value="">— pick an SFX file —</option>
        {sfxFiles.map((f) => (
          <option key={f.path} value={f.path}>
            {f.path}
          </option>
        ))}
      </select>
      <input
        value={name}
        onChange={(e) => setName(e.target.value)}
        placeholder="Display name"
        required
      />
      <input
        value={hotkey}
        onChange={(e) => setHotkey(e.target.value.slice(0, 1))}
        maxLength={1}
        placeholder="hotkey"
        title="Single-character hotkey (optional)"
      />
      <button type="submit" className="btn-primary" disabled={busy || !file || !name}>
        {busy ? "Adding…" : "+ Add"}
      </button>
    </form>
  );
}
