import { useCallback, useEffect, useState } from "react";
import type { FormEvent } from "react";

import { confirmDialog } from "@/components/confirmDialog";
import { IconButton } from "@/components/IconButton";
import { TrashIcon } from "@/components/icons";
import { NoModeEmpty } from "@/components/NoModeEmpty";
import { SoundboardEditor } from "@/components/SoundboardEditor";
import { modesAdminApi, modesApi } from "@/core/api";
import { usePlayerStore } from "@/core/playerStore";
import { toast } from "@/core/toast";
import type { ModeDetail, SoundboardManifest } from "@/core/types";

/** Authoring → Soundboards. The active mode's soundboards: list, create,
 *  delete, and edit (categories + items) via the shared SoundboardEditor. */
export function SoundboardsView() {
  const activeModeId = usePlayerStore((s) => s.state?.active_mode_id ?? null);
  const [boards, setBoards] = useState<SoundboardManifest[]>([]);
  const [selectedId, setSelectedId] = useState<string | null>(null);
  const [creating, setCreating] = useState(false);

  const load = useCallback(async () => {
    if (activeModeId === null) {
      setBoards([]);
      return;
    }
    try {
      const detail: ModeDetail = await modesApi.get(activeModeId);
      setBoards(Object.values(detail.soundboards));
    } catch (e) {
      toast.error("Load failed", e instanceof Error ? e.message : undefined);
    }
  }, [activeModeId]);

  useEffect(() => {
    void load();
  }, [load]);

  if (activeModeId === null) return <NoModeEmpty kind="Soundboards" />;

  async function remove(id: string) {
    if (activeModeId === null) return;
    const ok = await confirmDialog({
      title: `Delete soundboard "${id}"?`,
      tone: "danger",
      confirmLabel: "Delete",
    });
    if (!ok) return;
    try {
      await modesAdminApi.deleteSoundboard(activeModeId, id);
      toast.success("Soundboard deleted");
      if (selectedId === id) setSelectedId(null);
      await load();
    } catch (e) {
      toast.error("Delete failed", e instanceof Error ? e.message : undefined);
    }
  }

  if (selectedId !== null) {
    return (
      <SoundboardEditor
        modeId={activeModeId}
        soundboardId={selectedId}
        breadcrumb={[
          {
            label: "Soundboards",
            onClick: () => {
              setSelectedId(null);
              void load();
            },
          },
          { label: selectedId },
        ]}
      />
    );
  }

  return (
    <div className="two-pane-view soundboards-view">
      <div className="two-pane-pane">
        <header className="playlists-header">
          <h2>Soundboards</h2>
          <button
            type="button"
            className="btn-primary"
            onClick={() => setCreating((v) => !v)}
          >
            {creating ? "Cancel" : "+ New"}
          </button>
        </header>
        {creating ? (
          <SoundboardCreateForm
            existing={new Set(boards.map((b) => b.id))}
            onCreate={async (id, name) => {
              await modesAdminApi.createSoundboard(
                activeModeId,
                name ? { id, name } : { id },
              );
              setCreating(false);
              await load();
              setSelectedId(id);
            }}
          />
        ) : null}
        <ul className="playlist-list">
          {boards.length === 0 ? (
            <li className="muted small empty">No soundboards in this mode yet.</li>
          ) : (
            boards.map((b) => (
              <li key={b.id} className="playlist-list-item">
                <button
                  type="button"
                  className="playlist-list-item-meta btn-ghost"
                  onClick={() => setSelectedId(b.id)}
                >
                  <span className="playlist-name">{b.name || b.id}</span>
                  <span className="muted small">
                    {b.categories.reduce((acc, c) => acc + c.items.length, 0)} item
                    {b.categories.reduce((acc, c) => acc + c.items.length, 0) === 1
                      ? ""
                      : "s"}
                  </span>
                </button>
                <span className="simple-list-actions">
                  <IconButton
                    label="Delete soundboard"
                    icon={<TrashIcon />}
                    variant="danger"
                    onClick={() => void remove(b.id)}
                  />
                </span>
              </li>
            ))
          )}
        </ul>
      </div>
    </div>
  );
}

function SoundboardCreateForm({
  existing,
  onCreate,
}: {
  existing: Set<string>;
  onCreate: (id: string, name: string) => Promise<void>;
}) {
  const [id, setId] = useState("");
  const [name, setName] = useState("");
  const [busy, setBusy] = useState(false);

  async function submit(e: FormEvent) {
    e.preventDefault();
    if (existing.has(id.trim())) {
      toast.error("Soundboard id already exists");
      return;
    }
    setBusy(true);
    try {
      await onCreate(id.trim(), name.trim());
    } catch (err) {
      toast.error("Create failed", err instanceof Error ? err.message : undefined);
    } finally {
      setBusy(false);
    }
  }

  return (
    <form onSubmit={submit} className="inline-create-row">
      <input
        type="text"
        value={id}
        onChange={(e) => setId(e.target.value)}
        placeholder="id (slug)"
        pattern="[a-z0-9][a-z0-9_-]*"
        title="lowercase letters/digits with optional dashes/underscores"
        required
        autoFocus
      />
      <input
        type="text"
        value={name}
        onChange={(e) => setName(e.target.value)}
        placeholder="Name (optional)"
      />
      <button type="submit" className="btn-primary" disabled={busy}>
        Create
      </button>
    </form>
  );
}
