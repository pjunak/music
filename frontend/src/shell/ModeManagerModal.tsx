import { useState } from "react";
import type { FormEvent } from "react";

import { confirmDialog } from "@/components/confirmDialog";
import { inputDialog } from "@/components/inputDialog";
import { modesAdminApi } from "@/core/api";
import { uniqueSlug } from "@/core/slugify";
import { toast } from "@/core/toast";
import type { ModeSummary } from "@/core/types";
import { wsClient } from "@/core/ws";

/** Create / rename / delete modes — opened from the header button next to the
 *  mode picker. Modes own everything authored (playlists, soundboards, cues,
 *  EQ presets), so this is the one place to manage the campaign containers. */
export function ModeManagerModal({
  open,
  onClose,
  modes,
  activeModeId,
  onChanged,
}: {
  open: boolean;
  onClose: () => void;
  modes: ModeSummary[];
  activeModeId: string | null;
  onChanged: () => void | Promise<void>;
}) {
  const [name, setName] = useState("");
  const [busy, setBusy] = useState(false);

  if (!open) return null;

  // The on-disk slug is derived from the name — the operator never types an id.
  const derivedId = uniqueSlug(name, new Set(modes.map((m) => m.id)), "mode");

  async function create(e: FormEvent) {
    e.preventDefault();
    if (!name.trim()) return;
    setBusy(true);
    try {
      const created = await modesAdminApi.create(derivedId, name.trim());
      toast.success("Mode created", created.name);
      setName("");
      await onChanged();
      // Switch to the new mode so the (now mode-scoped) Authoring tabs land on it.
      wsClient.send({ type: "set_active_mode", mode_id: created.id });
    } catch (err) {
      toast.error("Create failed", err instanceof Error ? err.message : undefined);
    } finally {
      setBusy(false);
    }
  }

  async function rename(m: ModeSummary) {
    const next = await inputDialog({
      title: `Rename "${m.name}"`,
      label: "Display name",
      initial: m.name,
    });
    if (next === null || next.trim() === "" || next.trim() === m.name) return;
    try {
      await modesAdminApi.rename(m.id, next.trim());
      toast.success("Mode renamed");
      await onChanged();
    } catch (err) {
      toast.error("Rename failed", err instanceof Error ? err.message : undefined);
    }
  }

  async function remove(m: ModeSummary) {
    const ok = await confirmDialog({
      title: `Delete mode "${m.name}"?`,
      body: "The whole mode folder — its soundboards, cues, and EQ presets — is removed from disk. Playlists in this mode are orphaned. This can't be undone.",
      tone: "danger",
      confirmLabel: "Delete",
    });
    if (!ok) return;
    try {
      await modesAdminApi.delete(m.id);
      if (activeModeId === m.id) {
        wsClient.send({ type: "set_active_mode", mode_id: null });
      }
      toast.success("Mode deleted");
      await onChanged();
    } catch (err) {
      toast.error("Delete failed", err instanceof Error ? err.message : undefined);
    }
  }

  return (
    <div className="modal-backdrop" onMouseDown={onClose}>
      <div
        className="modal mode-manager"
        role="dialog"
        aria-modal="true"
        aria-label="Manage modes"
        onMouseDown={(e) => e.stopPropagation()}
      >
        <header className="modal-header">
          <h2>Manage modes</h2>
          <button type="button" onClick={onClose} aria-label="Close" title="Close">
            ×
          </button>
        </header>
        <div className="modal-body">
          <form className="mode-create-row" onSubmit={create}>
            <input
              value={name}
              onChange={(e) => setName(e.target.value)}
              placeholder="New mode name"
              required
              autoFocus
            />
            <button type="submit" className="btn-primary" disabled={busy || !name.trim()}>
              + Create
            </button>
          </form>

          {modes.length === 0 ? (
            <p className="muted small">No modes yet — create one above.</p>
          ) : (
            <ul className="mode-manage-list">
              {modes.map((m) => (
                <li key={m.id}>
                  <span className="mode-manage-name">
                    {m.name}
                    {m.id === activeModeId ? <span className="tag">active</span> : null}
                  </span>
                  <span className="muted small mode-manage-id">{m.id}</span>
                  <span className="mode-manage-actions">
                    <button type="button" onClick={() => void rename(m)}>
                      Rename
                    </button>
                    <button
                      type="button"
                      className="btn-danger"
                      onClick={() => void remove(m)}
                    >
                      Delete
                    </button>
                  </span>
                </li>
              ))}
            </ul>
          )}
        </div>
      </div>
    </div>
  );
}
