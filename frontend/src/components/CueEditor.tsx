import { useCallback, useEffect, useMemo, useState } from "react";
import type { FormEvent } from "react";

import { Breadcrumb } from "@/components/Breadcrumb";
import type { BreadcrumbItem } from "@/components/Breadcrumb";
import { Field } from "@/components/Field";
import { IconButton } from "@/components/IconButton";
import { XIcon } from "@/components/icons";
import { VolumeControl } from "@/components/VolumeControl";
import { modesAdminApi, modesApi, playlistsApi, presetsApi } from "@/core/api";
import type { PresetManifest } from "@/core/api";
import { toast } from "@/core/toast";
import type {
  Cue,
  CueLoop,
  CueSfx,
  ModeDetail,
  PlaylistMeta,
  SoundboardManifest,
  TrackInPlaylist,
} from "@/core/types";

interface Props {
  modeId: string;
  cueId: string;
  breadcrumb: BreadcrumbItem[];
}

/** Edit a cue: the preset it applies, the playlist it starts (from a song +
 *  timestamp), one-shot SFX, and loops. Loads via `modesApi.get`, saves via the
 *  PUT endpoint (full replace), and re-renders from the server response. */
export function CueEditor({ modeId, cueId, breadcrumb }: Props) {
  const [cue, setCue] = useState<Cue | null>(null);
  const [soundboards, setSoundboards] = useState<Record<string, SoundboardManifest>>(
    {},
  );
  const [presets, setPresets] = useState<PresetManifest[]>([]);
  const [playlists, setPlaylists] = useState<PlaylistMeta[]>([]);
  const [error, setError] = useState<string | null>(null);

  const refresh = useCallback(async () => {
    try {
      const detail: ModeDetail = await modesApi.get(modeId);
      const c = detail.cues[cueId] ?? null;
      setCue(c);
      setSoundboards(detail.soundboards);
      setError(c === null ? `Cue "${cueId}" not found.` : null);
    } catch (e) {
      setError(e instanceof Error ? e.message : "load failed");
    }
  }, [modeId, cueId]);

  useEffect(() => {
    void refresh();
  }, [refresh]);

  useEffect(() => {
    void presetsApi
      .list(modeId)
      .then(setPresets)
      .catch(() => setPresets([]));
    void playlistsApi
      .list({ mode_id: modeId })
      .then(setPlaylists)
      .catch(() => setPlaylists([]));
  }, [modeId]);

  if (error !== null) {
    return (
      <div className="cue-editor-empty">
        <Breadcrumb items={breadcrumb} />
        <p className="error small">{error}</p>
      </div>
    );
  }
  if (cue === null) {
    return (
      <div className="cue-editor-empty">
        <Breadcrumb items={breadcrumb} />
        <p className="muted small">Loading…</p>
      </div>
    );
  }
  return (
    <CueEditorForm
      modeId={modeId}
      cue={cue}
      soundboards={soundboards}
      presets={presets}
      playlists={playlists}
      breadcrumb={breadcrumb}
      onSaved={setCue}
    />
  );
}

function CueEditorForm({
  modeId,
  cue,
  soundboards,
  presets,
  playlists,
  breadcrumb,
  onSaved,
}: {
  modeId: string;
  cue: Cue;
  soundboards: Record<string, SoundboardManifest>;
  presets: PresetManifest[];
  playlists: PlaylistMeta[];
  breadcrumb: BreadcrumbItem[];
  onSaved: (c: Cue) => void;
}) {
  const [name, setName] = useState(cue.name);
  const [description, setDescription] = useState(cue.description ?? "");
  const [preset, setPreset] = useState(cue.preset ?? "");
  const [playlist, setPlaylist] = useState(cue.playlist ?? "");
  const [startIndex, setStartIndex] = useState(cue.start_index ?? 0);
  const [startMs, setStartMs] = useState(cue.start_ms ?? 0);
  const [sfx, setSfx] = useState<CueSfx[]>(() => (cue.sfx ?? []).map((s) => ({ ...s })));
  const [loops, setLoops] = useState<CueLoop[]>(
    () => (cue.loops ?? []).map((l) => ({ ...l })),
  );
  const [tracks, setTracks] = useState<TrackInPlaylist[]>([]);
  const [busy, setBusy] = useState(false);

  useEffect(() => {
    setName(cue.name);
    setDescription(cue.description ?? "");
    setPreset(cue.preset ?? "");
    setPlaylist(cue.playlist ?? "");
    setStartIndex(cue.start_index ?? 0);
    setStartMs(cue.start_ms ?? 0);
    setSfx((cue.sfx ?? []).map((s) => ({ ...s })));
    setLoops((cue.loops ?? []).map((l) => ({ ...l })));
  }, [cue]);

  // The selected playlist's tracks power the "from song" dropdown.
  const selectedPlaylistId = useMemo(
    () => playlists.find((p) => p.name === playlist)?.id ?? null,
    [playlists, playlist],
  );
  useEffect(() => {
    if (selectedPlaylistId === null) {
      setTracks([]);
      return;
    }
    let cancelled = false;
    void playlistsApi
      .tracks(selectedPlaylistId)
      .then((t) => {
        if (!cancelled) setTracks(t);
      })
      .catch(() => {
        if (!cancelled) setTracks([]);
      });
    return () => {
      cancelled = true;
    };
  }, [selectedPlaylistId]);

  async function submit(e: FormEvent) {
    e.preventDefault();
    setBusy(true);
    try {
      // A row with only one of soundboard/item set would be silently dropped by
      // the filter below, losing the operator's half-finished edit without a
      // word. Block the save so they can finish or remove it deliberately.
      const partialSfx = sfx.some((s) => Boolean(s.soundboard) !== Boolean(s.item));
      const partialLoops = loops.some(
        (l) => Boolean(l.soundboard) !== Boolean(l.item),
      );
      if (partialSfx || partialLoops) {
        toast.error(
          "Finish or remove incomplete SFX/loop rows",
          "Each row needs both a soundboard and an item.",
        );
        setBusy(false);
        return;
      }
      const body: Omit<Cue, "id"> = {
        name: name.trim(),
        description: description.trim() || null,
        preset: preset || null,
        playlist: playlist || null,
        start_index: playlist ? startIndex : 0,
        start_ms: playlist ? startMs : 0,
        sfx: sfx.filter((s) => s.soundboard && s.item),
        loops: loops.filter((l) => l.soundboard && l.item),
      };
      const updated = await modesAdminApi.updateCue(modeId, cue.id, body);
      toast.success("Cue saved");
      onSaved(updated);
    } catch (err) {
      toast.error("Save failed", err instanceof Error ? err.message : undefined);
    } finally {
      setBusy(false);
    }
  }

  return (
    <form onSubmit={submit} className="cue-editor">
      <Breadcrumb items={breadcrumb} />
      <header className="cue-editor-head">
        <h2>{cue.name || cue.id}</h2>
        <p className="muted small">Fire it live from the Console → Cues panel.</p>
      </header>

      <section className="surface-card authoring-card">
        <h3 className="section-label">Details</h3>
        <div className="field-row">
          <Field label="Name">
            <input
              type="text"
              value={name}
              onChange={(e) => setName(e.target.value)}
              required
            />
          </Field>
          <Field label="Description">
            <input
              type="text"
              value={description}
              onChange={(e) => setDescription(e.target.value)}
            />
          </Field>
        </div>
      </section>

      <section className="surface-card authoring-card">
        <h3 className="section-label">Sound</h3>
        <Field label="Apply preset">
          <select value={preset} onChange={(e) => setPreset(e.target.value)}>
            <option value="">— none —</option>
            {presets.map((p) => (
              <option key={p.id} value={p.id}>
                {p.name}
              </option>
            ))}
            {preset !== "" && !presets.some((p) => p.id === preset) ? (
              <option value={preset}>⚠ {preset} (missing)</option>
            ) : null}
          </select>
        </Field>
        <div className="cue-colour">
          <span className="field-label">Colour</span>
          <div className="segmented" role="group" aria-label="Colour target">
            <span className="segmented-item">Music</span>
            <span
              className="segmented-item seg-soon"
              aria-disabled="true"
              title="Colouring SFX isn't available yet — music only for now."
            >
              SFX
            </span>
            <span
              className="segmented-item seg-soon"
              aria-disabled="true"
              title="Colouring both isn't available yet — music only for now."
            >
              Both
            </span>
          </div>
          <span className="muted small">SFX / Both — soon</span>
        </div>
      </section>

      <section className="surface-card authoring-card">
        <h3 className="section-label">Music</h3>
        <Field label="Start playlist">
          <select
            value={playlist}
            onChange={(e) => {
              setPlaylist(e.target.value);
              // Reset the song/time anchors — a carried-over index from a
              // longer playlist would point past the end of a shorter one,
              // leaving the "From song" dropdown blank and persisting a stale
              // out-of-range start_index.
              setStartIndex(0);
              setStartMs(0);
            }}
          >
            <option value="">— leave music alone —</option>
            {playlists.map((p) => (
              <option key={p.id} value={p.name}>
                {p.name}
              </option>
            ))}
            {playlist !== "" && !playlists.some((p) => p.name === playlist) ? (
              <option value={playlist}>⚠ {playlist} (missing)</option>
            ) : null}
          </select>
        </Field>
        {playlist !== "" ? (
          <div className="field-row">
            <Field label="From song">
              <select
                value={startIndex}
                onChange={(e) => setStartIndex(Number(e.target.value))}
              >
                {tracks.length === 0 ? (
                  <option value={0}>start of playlist</option>
                ) : (
                  tracks.map((t, i) => (
                    <option key={i} value={i}>
                      {i + 1} · {trackLabel(t)}
                    </option>
                  ))
                )}
              </select>
            </Field>
            <Field label="At time (m:ss)">
              <input
                type="text"
                value={formatTime(startMs)}
                onChange={(e) => setStartMs(parseTime(e.target.value))}
                placeholder="0:00"
              />
            </Field>
          </div>
        ) : null}
      </section>

      <section className="surface-card authoring-card">
        <h3 className="section-label">
          One-shot SFX <span className="muted small">(fired once on run)</span>
        </h3>
        {sfx.length === 0 ? <p className="muted small">None.</p> : null}
        <ul className="simple-list">
          {sfx.map((s, idx) => (
            <li key={idx} className="looping-sfx-row">
              <SfxItemPicker
                soundboard={s.soundboard}
                item={s.item}
                soundboards={soundboards}
                onChange={(patch) =>
                  setSfx((prev) =>
                    prev.map((row, i) => (i === idx ? { ...row, ...patch } : row)),
                  )
                }
              />
              <VolumeControl
                value={s.volume ?? 1}
                onChange={(v) =>
                  setSfx((prev) =>
                    prev.map((row, i) => (i === idx ? { ...row, volume: v } : row)),
                  )
                }
                label="SFX volume"
                showIcon={false}
              />
              <IconButton
                label="Remove SFX"
                icon={<XIcon />}
                variant="danger"
                onClick={() => setSfx((prev) => prev.filter((_, i) => i !== idx))}
              />
            </li>
          ))}
        </ul>
        <button
          type="button"
          className="btn-secondary-soft"
          onClick={() => setSfx((prev) => [...prev, { soundboard: "", item: "" }])}
        >
          + Add SFX
        </button>
      </section>

      <section className="surface-card authoring-card">
        <h3 className="section-label">
          Loops <span className="muted small">(repeat until stopped)</span>
        </h3>
        {loops.length === 0 ? <p className="muted small">None.</p> : null}
        <ul className="simple-list">
          {loops.map((l, idx) => (
            <li key={idx} className="looping-sfx-row">
              <SfxItemPicker
                soundboard={l.soundboard}
                item={l.item}
                soundboards={soundboards}
                onChange={(patch) =>
                  setLoops((prev) =>
                    prev.map((row, i) => (i === idx ? { ...row, ...patch } : row)),
                  )
                }
              />
              <label className="loop-interval-field">
                <span className="muted small">every</span>
                <input
                  type="number"
                  min={1}
                  max={3600}
                  value={l.interval_s}
                  onChange={(e) =>
                    setLoops((prev) =>
                      prev.map((row, i) =>
                        i === idx
                          ? { ...row, interval_s: Number(e.target.value) }
                          : row,
                      ),
                    )
                  }
                />
                <span className="muted small">s</span>
              </label>
              <VolumeControl
                value={l.volume ?? 1}
                onChange={(v) =>
                  setLoops((prev) =>
                    prev.map((row, i) => (i === idx ? { ...row, volume: v } : row)),
                  )
                }
                label="Loop volume"
                showIcon={false}
              />
              <IconButton
                label="Remove loop"
                icon={<XIcon />}
                variant="danger"
                onClick={() => setLoops((prev) => prev.filter((_, i) => i !== idx))}
              />
            </li>
          ))}
        </ul>
        <button
          type="button"
          className="btn-secondary-soft"
          onClick={() =>
            setLoops((prev) => [
              ...prev,
              { soundboard: "", item: "", interval_s: 45 },
            ])
          }
        >
          + Add loop
        </button>
      </section>

      <div className="form-actions">
        <button type="submit" className="btn-primary" disabled={busy}>
          {busy ? "Saving…" : "Save changes"}
        </button>
      </div>
    </form>
  );
}

/** Cascading soundboard → item picker. Text fallback when the referenced
 *  soundboard/item isn't in the loaded mode so a hand-edited cue isn't
 *  silently blanked. */
function SfxItemPicker({
  soundboard,
  item,
  soundboards,
  onChange,
}: {
  soundboard: string;
  item: string;
  soundboards: Record<string, SoundboardManifest>;
  onChange: (patch: { soundboard?: string; item?: string }) => void;
}) {
  const entries = Object.values(soundboards);
  const selected = soundboard ? soundboards[soundboard] : undefined;
  const legacySoundboard = soundboard !== "" && selected === undefined;
  const legacyItem =
    item !== "" &&
    selected !== undefined &&
    !selected.categories.some((c) => c.items.some((it) => it.file === item));

  return (
    <>
      {legacySoundboard ? (
        <input
          type="text"
          value={soundboard}
          onChange={(e) => onChange({ soundboard: e.target.value })}
          title="This soundboard isn't in the loaded mode — edit as text."
        />
      ) : (
        <select
          value={soundboard}
          onChange={(e) => onChange({ soundboard: e.target.value, item: "" })}
          aria-label="Soundboard"
        >
          <option value="">— soundboard —</option>
          {entries.map((sb) => (
            <option key={sb.id} value={sb.id}>
              {sb.name ?? sb.id}
            </option>
          ))}
        </select>
      )}
      {selected === undefined || legacyItem ? (
        <input
          type="text"
          value={item}
          onChange={(e) => onChange({ item: e.target.value })}
          placeholder="item path (e.g. dnd/roar.ogg)"
          disabled={!soundboard}
        />
      ) : (
        <select
          value={item}
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
    </>
  );
}

function trackLabel(t: TrackInPlaylist): string {
  const title = t.track?.title?.trim();
  if (title) return title;
  const path = t.track?.path;
  if (path) return path.split("/").pop() ?? path;
  return `track ${t.track_id}`;
}

function formatTime(ms: number): string {
  const total = Math.max(0, Math.floor(ms / 1000));
  const m = Math.floor(total / 60);
  const s = total % 60;
  return `${m}:${String(s).padStart(2, "0")}`;
}

function parseTime(value: string): number {
  const v = value.trim();
  if (v === "") return 0;
  if (v.includes(":")) {
    const [m, s] = v.split(":");
    const mins = parseInt(m, 10) || 0;
    const secs = parseInt(s, 10) || 0;
    return (mins * 60 + secs) * 1000;
  }
  const secs = parseInt(v, 10) || 0;
  return secs * 1000;
}
