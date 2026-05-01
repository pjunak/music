import { useCallback, useEffect, useMemo, useState } from "react";
import type { FormEvent } from "react";

import { IconButton } from "@/components/IconButton";
import { LightningIcon, XIcon } from "@/components/icons";
import { VolumeControl } from "@/components/VolumeControl";
import { modesAdminApi, modesApi, playlistsApi, presetsApi } from "@/core/api";
import type { PresetManifest } from "@/core/api";
import { usePlayerStore } from "@/core/playerStore";
import { toast } from "@/core/toast";
import type {
  ModeDetail,
  PlaylistMeta,
  SceneLoopingSfx,
  SceneSpec,
  SoundboardManifest,
} from "@/core/types";
import { wsClient } from "@/core/ws";

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
  const [soundboards, setSoundboards] = useState<Record<string, SoundboardManifest>>(
    {},
  );

  const refresh = useCallback(async () => {
    try {
      const detail: ModeDetail = await modesApi.get(modeId);
      const s = detail.scenes[sceneId] ?? null;
      setScene(s);
      setSoundboards(detail.soundboards);
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
      soundboards={soundboards}
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
  soundboards,
  onBack,
  onSaved,
}: {
  modeId: string;
  scene: SceneSpec;
  presets: PresetManifest[];
  playlists: PlaylistMeta[];
  soundboards: Record<string, SoundboardManifest>;
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
  const [overrideVolume, setOverrideVolume] = useState<boolean>(
    typeof scene.volume === "number",
  );
  const [volume, setVolume] = useState<number>(
    typeof scene.volume === "number" ? scene.volume : 1,
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
    setOverrideVolume(typeof scene.volume === "number");
    setVolume(typeof scene.volume === "number" ? scene.volume : 1);
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
      if (overrideVolume) {
        payload.volume = volume;
      } else {
        payload.clear_volume = true;
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
          <SceneActivateButton modeId={modeId} sceneId={scene.id} />
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
        <h3>Master volume override</h3>
        <p className="muted small">
          Pin the master volume while the scene is active. Deactivating restores
          the previous volume. Leave off to keep whatever the operator was at.
        </p>
        <label className="checkbox-row">
          <input
            type="checkbox"
            checked={overrideVolume}
            onChange={(e) => setOverrideVolume(e.target.checked)}
          />
          <span>Override master volume</span>
        </label>
        {overrideVolume ? (
          <VolumeControl
            value={volume}
            onChange={setVolume}
            label="Scene master volume"
          />
        ) : null}
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
              <LoopingSfxRow
                key={idx}
                row={s}
                soundboards={soundboards}
                onChange={(patch) => updateLoopingSfx(idx, patch)}
                onRemove={() => removeLoopingSfx(idx)}
              />
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

/** One row in the looping-SFX list. Cascading dropdowns: pick a soundboard
 *  in this mode, then pick an item from that soundboard. The item dropdown
 *  is grouped by category so a "doors / wooden-creak" structure stays
 *  navigable when a soundboard has many items. The text-fallback path
 *  (legacy: row carries a soundboard/item not in the loaded mode) keeps
 *  the value visible + editable so a hand-edited YAML doesn't get silently
 *  blanked when the editor opens. */
function LoopingSfxRow({
  row,
  soundboards,
  onChange,
  onRemove,
}: {
  row: SceneLoopingSfx;
  soundboards: Record<string, SoundboardManifest>;
  onChange: (patch: Partial<SceneLoopingSfx>) => void;
  onRemove: () => void;
}) {
  const soundboardEntries = Object.values(soundboards);
  const selected = row.soundboard ? soundboards[row.soundboard] : undefined;
  const isLegacySoundboard =
    row.soundboard !== "" && selected === undefined;
  const isLegacyItem =
    row.item !== "" &&
    selected !== undefined &&
    !selected.categories.some((c) => c.items.some((it) => it.file === row.item));

  return (
    <li className="looping-sfx-row">
      {isLegacySoundboard ? (
        <input
          value={row.soundboard}
          onChange={(e) => onChange({ soundboard: e.target.value })}
          title="This soundboard isn't in the loaded mode — edit as text."
        />
      ) : (
        <select
          value={row.soundboard}
          onChange={(e) =>
            onChange({ soundboard: e.target.value, item: "" })
          }
          aria-label="Soundboard"
        >
          <option value="">— soundboard —</option>
          {soundboardEntries.map((sb) => (
            <option key={sb.id} value={sb.id}>
              {sb.name ?? sb.id}
            </option>
          ))}
        </select>
      )}

      {selected === undefined || isLegacyItem ? (
        <input
          value={row.item}
          onChange={(e) => onChange({ item: e.target.value })}
          placeholder="item path (e.g. dnd/torch.ogg)"
          disabled={!row.soundboard}
        />
      ) : (
        <select
          value={row.item}
          onChange={(e) => onChange({ item: e.target.value })}
          aria-label="Item"
        >
          <option value="">— item —</option>
          {selected.categories.map((cat) => (
            <optgroup key={cat.id} label={cat.name}>
              {cat.items.map((it) => (
                <option key={it.file} value={it.file}>
                  {it.name}
                </option>
              ))}
            </optgroup>
          ))}
        </select>
      )}

      <VolumeControl
        value={row.volume ?? 1}
        onChange={(v) => onChange({ volume: v })}
        label="Looping SFX volume"
        showIcon={false}
      />

      <IconButton
        label="Remove looping SFX"
        icon={<XIcon />}
        variant="danger"
        onClick={onRemove}
      />
    </li>
  );
}

/** "Try this scene" button — fires `activate_scene` so the operator hears
 *  the scene's audio (ambient + presets + looping SFX) without leaving the
 *  editor. Toggles to "Stop" when this scene is the currently-active one,
 *  so the same button serves both directions. Deliberately *not* a
 *  separate "preview" path — it's a real activation that the rest of the
 *  app sees. True transient preview (state stays unchanged) is on the
 *  roadmap; until then, hitting Stop instantly snaps back to whatever
 *  was active before. */
function SceneActivateButton({
  modeId,
  sceneId,
}: {
  modeId: string;
  sceneId: string;
}) {
  const activeModeId = usePlayerStore((s) => s.state?.active_mode_id ?? null);
  const activeSceneId = usePlayerStore((s) => s.state?.active_scene_id ?? null);
  const isThisActive = activeModeId === modeId && activeSceneId === sceneId;

  function trigger() {
    if (isThisActive) {
      wsClient.send({ type: "deactivate_scene" });
    } else {
      // Make sure we're activating the scene under its parent mode, not
      // whichever mode is currently active. Switching mode happens
      // server-side via SetActiveMode; we only fire it if needed.
      if (activeModeId !== modeId) {
        wsClient.send({ type: "set_active_mode", mode_id: modeId });
      }
      wsClient.send({ type: "activate_scene", scene_id: sceneId });
    }
  }

  return (
    <button
      type="button"
      className={isThisActive ? "btn-danger" : ""}
      onClick={trigger}
      title={
        isThisActive
          ? "Stop this scene and revert to the previous state"
          : "Activate this scene to hear it. Hit again to stop."
      }
    >
      {isThisActive ? "■ Stop scene" : (
        <>
          <LightningIcon /> Try scene
        </>
      )}
    </button>
  );
}
