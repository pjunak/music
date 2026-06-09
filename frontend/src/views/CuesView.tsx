import { useCallback, useEffect, useState } from "react";
import type { FormEvent } from "react";

import { confirmDialog } from "@/components/confirmDialog";
import { CueEditor } from "@/components/CueEditor";
import { IconButton } from "@/components/IconButton";
import { TrashIcon } from "@/components/icons";
import { NoModeEmpty } from "@/components/NoModeEmpty";
import { modesAdminApi, modesApi } from "@/core/api";
import { usePlayerStore } from "@/core/playerStore";
import { uniqueSlug } from "@/core/slugify";
import { toast } from "@/core/toast";
import type { Cue, ModeDetail } from "@/core/types";

/** Authoring → Cues. Lists the active mode's cues; edit opens the CueEditor. */
export function CuesView() {
  const activeModeId = usePlayerStore((s) => s.state?.active_mode_id ?? null);
  const [cues, setCues] = useState<Cue[]>([]);
  const [editingId, setEditingId] = useState<string | null>(null);
  const [creating, setCreating] = useState(false);

  const load = useCallback(async () => {
    if (activeModeId === null) {
      setCues([]);
      return;
    }
    try {
      const detail: ModeDetail = await modesApi.get(activeModeId);
      setCues(Object.values(detail.cues));
    } catch (e) {
      toast.error("Load failed", e instanceof Error ? e.message : undefined);
    }
  }, [activeModeId]);

  useEffect(() => {
    void load();
  }, [load]);

  if (activeModeId === null) return <NoModeEmpty kind="Cues" />;

  async function remove(id: string) {
    if (activeModeId === null) return;
    const ok = await confirmDialog({
      title: `Delete cue "${id}"?`,
      tone: "danger",
      confirmLabel: "Delete",
    });
    if (!ok) return;
    try {
      await modesAdminApi.deleteCue(activeModeId, id);
      toast.success("Cue deleted");
      if (editingId === id) setEditingId(null);
      await load();
    } catch (e) {
      toast.error("Delete failed", e instanceof Error ? e.message : undefined);
    }
  }

  if (editingId !== null) {
    return (
      <CueEditor
        modeId={activeModeId}
        cueId={editingId}
        breadcrumb={[
          {
            label: "Cues",
            onClick: () => {
              setEditingId(null);
              void load();
            },
          },
          { label: editingId },
        ]}
      />
    );
  }

  return (
    <div className="two-pane-view cues-view">
      <div className="two-pane-pane">
        <header className="playlists-header">
          <h2>Cues</h2>
          <button
            type="button"
            className="btn-primary"
            onClick={() => setCreating((v) => !v)}
          >
            {creating ? "Cancel" : "+ New"}
          </button>
        </header>
        <p className="muted small">
          A cue is a one-click setup — apply a preset, start a playlist from a
          song/time, fire SFX, start loops. Fire them from the Console.
        </p>
        {creating ? (
          <CueCreateForm
            existing={new Set(cues.map((c) => c.id))}
            onCreate={async (id, name) => {
              await modesAdminApi.createCue(activeModeId, { id, name });
              setCreating(false);
              await load();
              setEditingId(id);
            }}
          />
        ) : null}
        <ul className="playlist-list">
          {cues.length === 0 ? (
            <li className="muted small empty">No cues in this mode yet.</li>
          ) : (
            cues.map((c) => (
              <li key={c.id} className="playlist-list-item">
                <button
                  type="button"
                  className="playlist-list-item-meta btn-ghost"
                  onClick={() => setEditingId(c.id)}
                >
                  <span className="playlist-name">{c.name}</span>
                  <span className="muted small">id: {c.id}</span>
                </button>
                <span className="simple-list-actions">
                  <IconButton
                    label="Delete cue"
                    icon={<TrashIcon />}
                    variant="danger"
                    onClick={() => void remove(c.id)}
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

function CueCreateForm({
  existing,
  onCreate,
}: {
  existing: Set<string>;
  onCreate: (id: string, name: string) => Promise<void>;
}) {
  const [name, setName] = useState("");
  const [busy, setBusy] = useState(false);

  // The on-disk slug is derived from the name — no manual id entry.
  const derivedId = uniqueSlug(name, existing, "cue");

  async function submit(e: FormEvent) {
    e.preventDefault();
    if (!name.trim()) return;
    setBusy(true);
    try {
      await onCreate(derivedId, name.trim());
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
        value={name}
        onChange={(e) => setName(e.target.value)}
        placeholder="New cue name"
        aria-label="New cue name"
        required
        autoFocus
      />
      <button type="submit" className="btn-primary" disabled={busy || !name.trim()}>
        Create
      </button>
    </form>
  );
}
