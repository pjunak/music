import { useCallback, useEffect, useMemo, useState } from "react";
import type { FormEvent } from "react";

import { Breadcrumb } from "@/components/Breadcrumb";
import type { BreadcrumbItem } from "@/components/Breadcrumb";
import { confirmDialog } from "@/components/confirmDialog";
import { IconButton } from "@/components/IconButton";
import { TrashIcon, WarnIcon, XIcon } from "@/components/icons";
import { inputDialog } from "@/components/inputDialog";
import { modesAdminApi, modesApi, sfxApi } from "@/core/api";
import type { SfxFile } from "@/core/api";
import { uniqueSlug } from "@/core/slugify";
import { toast } from "@/core/toast";
import type { SoundboardManifest } from "@/core/types";

interface Props {
  modeId: string;
  soundboardId: string;
  /** Breadcrumb supplied by the host so it can express the ancestor chain
   *  in its own terms (the Authoring → Soundboards tab uses
   *  "Soundboards › Item"). The editor itself appends nothing — the leaf
   *  label is the host's responsibility too. */
  breadcrumb: BreadcrumbItem[];
}

/** Edit a single soundboard's categories and items. Loads the latest copy
 *  from `modesApi.get(modeId)`, applies edits via the admin endpoints, and
 *  re-renders from the response of each mutation. */
export function SoundboardEditor({
  modeId,
  soundboardId,
  breadcrumb,
}: Props) {
  const [soundboard, setSoundboard] = useState<SoundboardManifest | null>(null);
  const [sfxFiles, setSfxFiles] = useState<SfxFile[]>([]);
  const [error, setError] = useState<string | null>(null);

  const conflictHotkeys = useMemo(
    () => (soundboard ? collectConflictHotkeys(soundboard) : new Set<string>()),
    [soundboard],
  );

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
    const name = await inputDialog({
      title: "New category",
      label: "Category name",
      placeholder: "Doors",
      confirmLabel: "Add",
    });
    if (name === null) return;
    // Derive the on-disk slug from the name (the operator never types an id).
    const existing = new Set((soundboard?.categories ?? []).map((c) => c.id));
    const id = uniqueSlug(name, existing, "category");
    try {
      const updated = await modesAdminApi.addCategory(modeId, soundboardId, {
        id,
        name,
      });
      setSoundboard(updated);
      toast.success("Category added", name);
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
      const updated = await modesAdminApi.deleteCategory(
        modeId,
        soundboardId,
        categoryId,
      );
      setSoundboard(updated);
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
      setSoundboard(updated);
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
      setSoundboard(updated);
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
      setSoundboard(updated);
      toast.success("SFX removed");
    } catch (e) {
      toast.error("Remove failed", e instanceof Error ? e.message : undefined);
    }
  }

  if (error !== null) {
    return (
      <div className="soundboard-editor">
        <Breadcrumb items={breadcrumb} />
        <p className="error small">{error}</p>
      </div>
    );
  }

  if (soundboard === null) {
    return <p className="muted small">Loading…</p>;
  }

  return (
    <div className="soundboard-editor">
      <Breadcrumb items={breadcrumb} />
      <header className="playlist-detail-header">
        <div>
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
        <>
          {/* The runtime (`useSfxHotkeys`) silently picks the last-registered
              item when two share a key, which makes "why did pressing 5 play
              the wrong thing?" basically un-debuggable. Surface the duplicates
              here at edit time so the operator can fix it before live use. */}
          {conflictHotkeys.size > 0 ? (
            <div className="alert alert-warn">
              <WarnIcon aria-hidden="true" />
              <span>
                Hotkey conflict:{" "}
                {Array.from(conflictHotkeys).map((k) => `"${k}"`).join(", ")}{" "}
                {conflictHotkeys.size === 1 ? "is" : "are"} bound to multiple items.
                Only the last-registered item will fire when pressed.
              </span>
            </div>
          ) : null}
        <ul className="soundboard-categories">
          {soundboard.categories.map((cat) => (
            <li key={cat.id} className="soundboard-category">
              <header>
                <div>
                  <h3>{cat.name}</h3>
                  <span className="muted small">id: {cat.id}</span>
                </div>
                <IconButton
                  label="Remove category and all its items"
                  icon={<TrashIcon />}
                  variant="danger"
                  onClick={() => void removeCategory(cat.id)}
                >
                  Category
                </IconButton>
              </header>

              <ItemList
                items={cat.items}
                conflictHotkeys={conflictHotkeys}
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
        </>
      )}
    </div>
  );
}

/** Compute the set of hotkeys assigned to more than one item across the
 *  whole soundboard. Used for the conflict banner + the per-row warning. */
function collectConflictHotkeys(sb: SoundboardManifest): Set<string> {
  const counts = new Map<string, number>();
  for (const cat of sb.categories) {
    for (const it of cat.items) {
      if (!it.hotkey) continue;
      counts.set(it.hotkey, (counts.get(it.hotkey) ?? 0) + 1);
    }
  }
  return new Set(
    Array.from(counts.entries())
      .filter(([, n]) => n > 1)
      .map(([k]) => k),
  );
}

interface AddItemPayload {
  file: string;
  name: string;
  hotkey?: string;
}

function ItemList({
  items,
  conflictHotkeys,
  onUpdate,
  onDelete,
}: {
  items: SoundboardManifest["categories"][number]["items"];
  conflictHotkeys: Set<string>;
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
          hasHotkeyConflict={
            item.hotkey ? conflictHotkeys.has(item.hotkey) : false
          }
          onSave={(payload) => onUpdate(idx, payload)}
          onDelete={() => onDelete(idx)}
        />
      ))}
    </ol>
  );
}

function ItemRow({
  item,
  hasHotkeyConflict,
  onSave,
  onDelete,
}: {
  item: SoundboardManifest["categories"][number]["items"][number];
  hasHotkeyConflict: boolean;
  onSave: (payload: { name?: string; hotkey?: string }) => Promise<void>;
  onDelete: () => Promise<void>;
}) {
  const [name, setName] = useState(item.name);
  const [hotkey, setHotkey] = useState(item.hotkey ?? "");
  const [saving, setSaving] = useState(false);

  useEffect(() => {
    setName(item.name);
    setHotkey(item.hotkey ?? "");
  }, [item.name, item.hotkey]);

  const dirty = name !== item.name || (hotkey || null) !== (item.hotkey ?? null);

  async function save() {
    if (saving) return;
    setSaving(true);
    try {
      const payload: { name?: string; hotkey?: string } = { name };
      payload.hotkey = hotkey;
      await onSave(payload);
    } finally {
      setSaving(false);
    }
  }

  return (
    <li className="soundboard-item">
      <code className="soundboard-item-file" title={item.file}>
        {item.file}
      </code>
      <input
        className="soundboard-item-name"
        type="text"
        aria-label="Display name"
        value={name}
        onChange={(e) => setName(e.target.value)}
        placeholder="Display name"
      />
      <input
        className={`soundboard-item-hotkey${hasHotkeyConflict ? " has-conflict" : ""}`}
        type="text"
        aria-label="Hotkey"
        value={hotkey}
        onChange={(e) => setHotkey(e.target.value.slice(0, 1))}
        maxLength={1}
        placeholder="key"
        title={
          hasHotkeyConflict
            ? "This hotkey is bound to another item too - only the last-registered one will fire."
            : "Single-character keyboard hotkey, fired from anywhere when this soundboard is active"
        }
      />
      <div className="soundboard-item-actions">
        <button
          type="button"
          className={dirty ? "btn-primary" : ""}
          onClick={() => void save()}
          disabled={!dirty || saving}
        >
          Save
        </button>
        <IconButton
          label="Delete this item"
          icon={<XIcon />}
          variant="danger"
          onClick={() => void onDelete()}
        />
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
        type="text"
        aria-label="Display name"
        value={name}
        onChange={(e) => setName(e.target.value)}
        placeholder="Display name"
        required
      />
      <input
        type="text"
        aria-label="Hotkey (optional)"
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
