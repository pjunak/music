import { useCallback, useEffect, useMemo, useState } from "react";
import type { FormEvent } from "react";

import { IconButton } from "@/components/IconButton";
import { XIcon } from "@/components/icons";
import { modesAdminApi, modesApi, playlistsApi, presetsApi } from "@/core/api";
import type { PresetManifest } from "@/core/api";
import { toast } from "@/core/toast";
import type {
  ModeDetail,
  PlaylistMeta,
  SceneLoopingSfx,
  SceneSpec,
} from "@/core/types";

interface Props {
  modeId: string;
  sceneId: string;
  onBack: () => void;
}

/** Edit a scene's contents: name/description, ambient (playlist + crossfade),
 *  active presets, and looping SFX. The lights and external blocks remain
 *  raw YAML for now since they're integration-shaped and the integrations
 *  module is unwired.
 *
 *  Loads the latest copy via `modesApi.get(modeId)`, persists changes via the
 *  PATCH endpoint, and re-renders from the server response so backend-side
 *  cleanups (empty blocks dropped, etc.) show up immediately. */
export function SceneEditor({ modeId, sceneId, onBack }: Props) {
  const [scene, setScene] = useState<SceneSpec | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [presets, setPresets] = useState<PresetManifest[]>([]);
  const [playlists, setPlaylists] = useState<PlaylistMeta[]>([]);

  const refresh = useCallback(async () => {
    try {
      const detail: ModeDetail = await modesApi.get(modeId);
      const s = detail.scenes[sceneId] ?? null;
      setScene(s);
      setError(s === null ? `Scene "${sceneId}" not found.` : null);
    } catch (e) {
      setError(e instanceof Error ? e.message : "load failed");
    }
  }, [modeId, sceneId]);

  useEffect(() => {
    void refresh();
  }, [refresh]);

  useEffect(() => {
    void presetsApi
      .list()
      .then(setPresets)
      .catch(() => setPresets([]));
    void playlistsApi
      .list({ mode_id: modeId })
      .then(setPlaylists)
      .catch(() => setPlaylists([]));
  }, [modeId]);

  if (error !== null) {
    return (
      <div className="empty-detail">
        <p className="error small">{error}</p>
        <button type="button" onClick={onBack}>
          ← Back to mode
        </button>
      </div>
    );
  }
  if (scene === null) {
    return <p className="muted small">Loading…</p>;
  }
  return (
    <SceneEditorForm
      modeId={modeId}
      scene={scene}
      presets={presets}
      playlists={playlists}
      onBack={onBack}
      onSaved={(updated) => setScene(updated)}
    />
  );
}


function SceneEditorForm({
  modeId,
  scene,
  presets,
  playlists,
  onBack,
  onSaved,
}: {
  modeId: string;
  scene: SceneSpec;
  presets: PresetManifest[];
  playlists: PlaylistMeta[];
  onBack: () => void;
  onSaved: (s: SceneSpec) => void;
}) {
  const [name, setName] = useState(scene.name);
  const [description, setDescription] = useState(scene.description ?? "");
  const [ambientPlaylist, setAmbientPlaylist] = useState(
    scene.ambient?.playlist ?? "",
  );
  const [ambientCrossfadeMs, setAmbientCrossfadeMs] = useState(
    scene.ambient?.crossfade_ms ?? 0,
  );
  const [activePresets, setActivePresets] = useState<string[]>(
    scene.presets ?? [],
  );
  const [loopingSfx, setLoopingSfx] = useState<SceneLoopingSfx[]>(
    () => (scene.looping_sfx ?? []).map((s) => ({ ...s })),
  );
  const [busy, setBusy] = useState(false);

  // Re-sync local form when the parent's scene prop is replaced (after save).
  useEffect(() => {
    setName(scene.name);
    setDescription(scene.description ?? "");
    setAmbientPlaylist(scene.ambient?.playlist ?? "");
    setAmbientCrossfadeMs(scene.ambient?.crossfade_ms ?? 0);
    setActivePresets(scene.presets ?? []);
    setLoopingSfx((scene.looping_sfx ?? []).map((s) => ({ ...s })));
  }, [scene]);

  const playlistOptions = useMemo(
    () =>
      playlists.map((p) => ({
        value: p.name,
        label: `${p.name}${p.category ? ` (${p.category})` : ""}`,
      })),
    [playlists],
  );

  function togglePreset(id: string) {
    setActivePresets((prev) =>
      prev.includes(id) ? prev.filter((p) => p !== id) : [...prev, id],
    );
  }

  function addLoopingSfx() {
    setLoopingSfx((prev) => [...prev, { soundboard: "", item: "" }]);
  }
  function updateLoopingSfx(idx: number, patch: Partial<SceneLoopingSfx>) {
    setLoopingSfx((prev) =>
      prev.map((s, i) => (i === idx ? { ...s, ...patch } : s)),
    );
  }
  function removeLoopingSfx(idx: number) {
    setLoopingSfx((prev) => prev.filter((_, i) => i !== idx));
  }

  async function submit(e: FormEvent) {
    e.preventDefault();
    setBusy(true);
    try {
      // Build a payload that's explicit about ambient: if no playlist is
      // selected, ask the server to drop the block entirely so YAML stays
      // tidy. Same for presets and looping_sfx — empty arrays clear them.
      const payload: Parameters<typeof modesAdminApi.updateScene>[2] = {
        name: name.trim(),
        description: description.trim(),
        presets: activePresets,
        looping_sfx: loopingSfx
          .filter((s) => s.soundboard.trim() && s.item.trim())
          .map((s) => ({
            soundboard: s.soundboard.trim(),
            item: s.item.trim(),
            ...(typeof s.volume === "number" ? { volume: s.volume } : {}),
          })),
      };
      if (ambientPlaylist.trim()) {
        payload.ambient = {
          playlist: ambientPlaylist.trim(),
          ...(ambientCrossfadeMs > 0
            ? { crossfade_ms: ambientCrossfadeMs }
            : {}),
        };
      } else {
        payload.clear_ambient = true;
      }
      const updated = await modesAdminApi.updateScene(modeId, scene.id, payload);
      toast.success("Scene saved");
      onSaved(updated);
    } catch (err) {
      toast.error("Save failed", err instanceof Error ? err.message : undefined);
    } finally {
      setBusy(false);
    }
  }

  return (
    <form onSubmit={submit} className="scene-editor">
      <header className="playlist-detail-header">
        <div>
          <h2>Scene · {scene.id}</h2>
          <p className="muted small">
            Edit ambient music, presets, and looping SFX. Lights and external
            integrations stay in YAML for now.
          </p>
        </div>
        <div className="playlist-detail-actions">
          <button type="button" onClick={onBack}>
            ← Back to mode
          </button>
        </div>
      </header>

      <div className="playlist-meta-fields">
        <label>
          <span className="muted small">Name</span>
          <input
            value={name}
            onChange={(e) => setName(e.target.value)}
            required
          />
        </label>
        <label>
          <span className="muted small">Description</span>
          <input
            value={description}
            onChange={(e) => setDescription(e.target.value)}
          />
        </label>
      </div>

      <section>
        <h3>Ambient</h3>
        <p className="muted small">
          When the scene activates, the ambient lane plays this playlist with
          the given crossfade. Leave the playlist blank to keep ambient unchanged.
        </p>
        <div className="playlist-meta-fields">
          <label>
            <span className="muted small">Playlist (mode-scoped or global)</span>
            <input
              value={ambientPlaylist}
              onChange={(e) => setAmbientPlaylist(e.target.value)}
              list="scene-playlist-options"
              placeholder="(none)"
            />
            <datalist id="scene-playlist-options">
              {playlistOptions.map((p) => (
                <option key={p.value} value={p.value}>
                  {p.label}
                </option>
              ))}
            </datalist>
          </label>
          <label>
            <span className="muted small">Crossfade (ms)</span>
            <input
              type="number"
              min={0}
              max={30000}
              step={100}
              value={ambientCrossfadeMs}
              onChange={(e) =>
                setAmbientCrossfadeMs(parseInt(e.target.value, 10) || 0)
              }
              disabled={!ambientPlaylist.trim()}
            />
          </label>
        </div>
      </section>

      <section>
        <h3>Active presets</h3>
        {presets.length === 0 ? (
          <p className="muted small">No presets installed yet.</p>
        ) : (
          <div className="presets-grid">
            {presets.map((p) => {
              const on = activePresets.includes(p.id);
              return (
                <button
                  key={p.id}
                  type="button"
                  className={`preset-chip${on ? " active" : ""}`}
                  onClick={() => togglePreset(p.id)}
                  title={p.description ?? undefined}
                >
                  {p.name}
                </button>
              );
            })}
          </div>
        )}
      </section>

      <section>
        <h3>Looping SFX</h3>
        <p className="muted small">
          One-shot SFX events emitted on activation. The frontend doesn't loop
          them yet — see the integration roadmap.
        </p>
        {loopingSfx.length === 0 ? (
          <p className="muted small">None.</p>
        ) : (
          <ul className="simple-list">
            {loopingSfx.map((s, idx) => (
              <li key={idx} className="looping-sfx-row">
                <input
                  value={s.soundboard}
                  onChange={(e) =>
                    updateLoopingSfx(idx, { soundboard: e.target.value })
                  }
                  placeholder="soundboard id"
                />
                <input
                  value={s.item}
                  onChange={(e) =>
                    updateLoopingSfx(idx, { item: e.target.value })
                  }
                  placeholder="item path (e.g. dnd/torch.ogg)"
                />
                <input
                  type="number"
                  min={0}
                  max={1}
                  step={0.05}
                  value={s.volume ?? 1}
                  onChange={(e) =>
                    updateLoopingSfx(idx, {
                      volume: parseFloat(e.target.value),
                    })
                  }
                  title="Volume 0–1"
                />
                <IconButton
                  label="Remove looping SFX"
                  icon={<XIcon />}
                  variant="danger"
                  onClick={() => removeLoopingSfx(idx)}
                />
              </li>
            ))}
          </ul>
        )}
        <button type="button" onClick={addLoopingSfx}>
          + Add SFX
        </button>
      </section>

      <div className="modal-actions">
        <button type="button" onClick={onBack} disabled={busy}>
          Cancel
        </button>
        <button type="submit" className="btn-primary" disabled={busy}>
          {busy ? "Saving…" : "Save changes"}
        </button>
      </div>
    </form>
  );
}
