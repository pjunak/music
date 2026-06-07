import { useEffect, useMemo, useState } from "react";
import type { FormEvent } from "react";

import { confirmDialog } from "@/components/confirmDialog";
import { IconButton } from "@/components/IconButton";
import { EditIcon, LightningIcon, TrashIcon } from "@/components/icons";
import { VolumeControl } from "@/components/VolumeControl";
import { modesAdminApi, playlistsApi } from "@/core/api";
import { toast } from "@/core/toast";
import type { InterruptSpec, ModeDetail, PlaylistMeta } from "@/core/types";
import { wsClient } from "@/core/ws";

/** Interrupt templates for one mode — list + create/edit/delete + fire.
 *  Extracted from the old Modes tab so it can be the Authoring → Interrupts
 *  sub-tab. Playlists are mode-scoped now (no global fallback). */
export function InterruptTemplatesEditor({
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

  // Playlists this mode's interrupts can reference (mode-scoped). Held here so
  // the form offers a real picker and the list flags a reference that no longer
  // resolves — instead of failing silently at fire.
  const [playlists, setPlaylists] = useState<PlaylistMeta[]>([]);
  useEffect(() => {
    let cancelled = false;
    void playlistsApi
      .list({ mode_id: modeId })
      .then((all) => {
        if (!cancelled) setPlaylists(all);
      })
      .catch(() => {
        if (!cancelled) setPlaylists([]);
      });
    return () => {
      cancelled = true;
    };
  }, [modeId]);
  const playlistNames = useMemo(
    () => new Set(playlists.map((p) => p.name)),
    [playlists],
  );

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

  function fire(spec: InterruptSpec) {
    if (spec.playlist) {
      const found = playlists.find((p) => p.name === spec.playlist) ?? null;
      if (found === null) {
        toast.error(
          "Playlist not found",
          `No playlist named "${spec.playlist ?? ""}" in this mode — re-pick it in the template.`,
        );
        return;
      }
      wsClient.send({
        type: "fire_interrupt_playlist",
        playlist_id: found.id,
        return_to_ambient: spec.return_to_ambient ?? true,
        fade_in_ms: spec.fade_in_ms ?? 0,
        fade_out_ms: spec.fade_out_ms ?? 0,
        duck_to: spec.duck_to ?? null,
      });
      toast.info("Interrupt fired", spec.name);
      return;
    }
    if (spec.soundboard_item) {
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
          playlists={playlists}
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
                  playlists={playlists}
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
                  {it.playlist ? (
                    <>
                      {" "}· playlist <code>{it.playlist}</code>
                      {!playlistNames.has(it.playlist) ? (
                        <span
                          className="ref-missing"
                          title="No playlist with this name in this mode — firing this interrupt will fail. Edit it to re-pick."
                        >
                          {" "}⚠ missing
                        </span>
                      ) : null}
                    </>
                  ) : null}
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
                  {typeof it.duck_to === "number" ? (
                    <span className="muted small">
                      {" "}
                      · ducks ambient to {Math.round(it.duck_to * 100)}%
                    </span>
                  ) : null}
                </span>
                <span className="simple-list-actions">
                  <IconButton
                    label="Fire now"
                    icon={<LightningIcon />}
                    onClick={() => fire(it)}
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
  playlists,
  onClose,
  onSaved,
}: {
  modeId: string;
  mode: "create" | "edit";
  index?: number;
  initial?: InterruptSpec;
  playlists: PlaylistMeta[];
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
  const [duckEnabled, setDuckEnabled] = useState(
    initial?.duck_to !== undefined && initial?.duck_to !== null,
  );
  const [duckLevel, setDuckLevel] = useState(
    typeof initial?.duck_to === "number" ? initial.duck_to : 0.3,
  );
  const [busy, setBusy] = useState(false);

  async function submit(e: FormEvent) {
    e.preventDefault();
    setBusy(true);
    try {
      const duckTo = duckEnabled ? duckLevel : null;
      if (mode === "create") {
        const payload: Parameters<typeof modesAdminApi.addInterrupt>[1] = {
          name: name.trim(),
          fade_in_ms: fadeInMs,
          fade_out_ms: fadeOutMs,
          return_to_ambient: returnToAmbient,
          duck_to: duckTo,
        };
        if (source === "playlist") payload.playlist = playlist.trim();
        else payload.soundboard_item = soundboardItem.trim();
        await modesAdminApi.addInterrupt(modeId, payload);
        toast.success("Interrupt added");
      } else if (index !== undefined) {
        const payload: Parameters<typeof modesAdminApi.updateInterrupt>[2] = {
          name: name.trim(),
          fade_in_ms: fadeInMs,
          fade_out_ms: fadeOutMs,
          return_to_ambient: returnToAmbient,
          duck_to: duckTo,
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
          <select value={source} onChange={(e) => setSource(e.target.value as Source)}>
            <option value="playlist">Playlist (by name)</option>
            <option value="soundboard_item">Soundboard item (file path)</option>
          </select>
        </label>
        {source === "playlist" ? (
          <label>
            <span className="muted small">Playlist</span>
            <select
              value={playlist}
              onChange={(e) => setPlaylist(e.target.value)}
              required
            >
              <option value="" disabled>
                Pick a playlist…
              </option>
              {playlists.map((p) => (
                <option key={p.id} value={p.name}>
                  {p.name}
                </option>
              ))}
              {playlist !== "" && !playlists.some((p) => p.name === playlist) ? (
                <option value={playlist}>⚠ {playlist} (missing)</option>
              ) : null}
            </select>
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
        <label className="checkbox-row">
          <input
            type="checkbox"
            checked={duckEnabled}
            onChange={(e) => setDuckEnabled(e.target.checked)}
          />
          <span>Duck ambient under the interrupt (cinematic) instead of pausing</span>
        </label>
        {duckEnabled ? (
          <label>
            <span className="muted small">Duck level (lower = quieter)</span>
            <VolumeControl
              value={duckLevel}
              onChange={setDuckLevel}
              label="Duck level"
              showIcon={false}
            />
          </label>
        ) : null}
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
