import { useCallback, useEffect, useState } from "react";
import type { FormEvent } from "react";

import { confirmDialog } from "@/components/confirmDialog";
import { EmptyState } from "@/components/EmptyState";
import { Fader } from "@/components/Fader";
import { GraphicEqModule } from "@/components/GraphicEqModule";
import { IconButton } from "@/components/IconButton";
import { ArrowDownIcon, ArrowUpIcon, TrashIcon, XIcon } from "@/components/icons";
import { Knob } from "@/components/Knob";
import { modesAdminApi, presetsAdminApi, presetsApi } from "@/core/api";
import type { PresetEffect, PresetManifest } from "@/core/api";
import { defaultEqBands, normalizeEqBands } from "@/core/eq";
import type { EqBand } from "@/core/eq";
import { usePlayerStore } from "@/core/playerStore";
import { toast } from "@/core/toast";

// Per-effect-type UI: a friendly label, a one-line blurb, and a control schema
// for each numeric param. Param KEYS must match what the audio engine reads
// (playbackEngine `buildEffect`) — only the presentation changes here. The
// graphic EQ (`eq`) is handled separately (GraphicEqModule). Effect types in
// neither place are unsupported (e.g. `pitch_shift`) and get flagged.
interface ParamControl {
  key: string;
  label: string;
  min: number;
  max: number;
  step: number;
  def: number;
  format: (v: number) => string;
  /** Log feel for wide frequency ranges. */
  scale?: "linear" | "log";
}
const pct = (v: number) => `${Math.round(v * 100)}%`;
const hz = (v: number) => (v >= 1000 ? `${(v / 1000).toFixed(1)}k` : `${Math.round(v)}`);
const sec = (v: number) => `${v.toFixed(2)}s`;
const plain = (v: number) => `${v}`;

const EQ_META = { label: "Graphic EQ", blurb: "10-band — drag the bands" };

const EFFECT_UI: Record<
  string,
  { label: string; blurb: string; params: ParamControl[] }
> = {
  lowpass: {
    label: "Muffle (low-pass)",
    blurb: "Cuts the highs — distant / underwater",
    params: [
      { key: "frequency", label: "Cutoff", min: 50, max: 18000, step: 10, def: 800, format: hz, scale: "log" },
      { key: "q", label: "Reso", min: 0.1, max: 12, step: 0.1, def: 0.7, format: plain },
    ],
  },
  highpass: {
    label: "Thin (high-pass)",
    blurb: "Cuts the lows — tinny / telephone",
    params: [
      { key: "frequency", label: "Cutoff", min: 50, max: 18000, step: 10, def: 200, format: hz, scale: "log" },
      { key: "q", label: "Reso", min: 0.1, max: 12, step: 0.1, def: 0.7, format: plain },
    ],
  },
  bandpass: {
    label: "Band-pass",
    blurb: "Keeps a band around the centre",
    params: [
      { key: "frequency", label: "Centre", min: 50, max: 18000, step: 10, def: 1000, format: hz, scale: "log" },
      { key: "q", label: "Width", min: 0.1, max: 12, step: 0.1, def: 1.0, format: plain },
    ],
  },
  delay: {
    label: "Echo",
    blurb: "Repeats with feedback",
    params: [
      { key: "time", label: "Time", min: 0, max: 2, step: 0.01, def: 0.25, format: sec },
      { key: "feedback", label: "Feedback", min: 0, max: 0.95, step: 0.01, def: 0.3, format: pct },
      { key: "wet", label: "Mix", min: 0, max: 1, step: 0.01, def: 0.4, format: pct },
    ],
  },
  distortion: {
    label: "Distortion",
    blurb: "Soft-clip grit",
    params: [
      { key: "amount", label: "Amount", min: 0, max: 100, step: 1, def: 50, format: plain },
    ],
  },
  tremolo: {
    label: "Tremolo",
    blurb: "Volume wobble",
    params: [
      { key: "rate", label: "Rate", min: 0.1, max: 20, step: 0.1, def: 5, format: (v) => `${v.toFixed(1)}Hz` },
      { key: "depth", label: "Depth", min: 0, max: 1, step: 0.01, def: 0.5, format: pct },
    ],
  },
  reverb: {
    label: "Reverb",
    blurb: "Room / space tail",
    params: [
      { key: "decay", label: "Size", min: 0.1, max: 8, step: 0.1, def: 2.0, format: sec },
      { key: "wet", label: "Mix", min: 0, max: 1, step: 0.01, def: 0.4, format: pct },
    ],
  },
};

// Order shown in the "+ Add" picker — EQ first, then the colour/space effects.
const ADDABLE: { type: string; label: string; blurb: string }[] = [
  { type: "eq", ...EQ_META },
  ...Object.entries(EFFECT_UI).map(([type, ui]) => ({
    type,
    label: ui.label,
    blurb: ui.blurb,
  })),
];

export function PresetsView() {
  // EQ presets are per-mode — this view edits the active mode's presets.
  const activeModeId = usePlayerStore((s) => s.state?.active_mode_id ?? null);
  const [presets, setPresets] = useState<PresetManifest[]>([]);
  const [selectedId, setSelectedId] = useState<string | null>(null);
  const [creating, setCreating] = useState(false);

  const refresh = useCallback(async () => {
    if (activeModeId === null) {
      setPresets([]);
      return;
    }
    try {
      const list = await presetsApi.list(activeModeId);
      setPresets(list);
      if (selectedId !== null && !list.some((p) => p.id === selectedId)) {
        setSelectedId(null);
      }
    } catch (e) {
      toast.error("Load failed", e instanceof Error ? e.message : undefined);
    }
  }, [selectedId, activeModeId]);

  useEffect(() => {
    void refresh();
  }, [refresh]);

  async function reloadPresets() {
    try {
      const result = await modesAdminApi.reload();
      const loaded = result.loaded.length;
      const errs = Object.entries(result.errors);
      if (errs.length === 0) {
        toast.success(`Reloaded ${loaded} mode${loaded === 1 ? "" : "s"} from disk`);
      } else {
        const sample = errs
          .slice(0, 3)
          .map(([id, err]) => `${id}: ${err}`)
          .join("\n");
        const more = errs.length > 3 ? `\n…and ${errs.length - 3} more` : "";
        toast.warn(
          `Reloaded ${loaded}, ${errs.length} error${errs.length === 1 ? "" : "s"}`,
          `${sample}${more}`,
        );
      }
      await refresh();
    } catch (e) {
      toast.error("Reload failed", e instanceof Error ? e.message : undefined);
    }
  }

  const selected = presets.find((p) => p.id === selectedId) ?? null;

  if (activeModeId === null) {
    return (
      <div className="empty-detail">
        <EmptyState title="No mode selected">
          EQ presets live inside a mode. Pick or create a mode from the header to
          edit its presets.
        </EmptyState>
      </div>
    );
  }

  return (
    <div className="two-pane-view presets-view">
      <div className="two-pane-pane presets-list-pane">
        <header className="playlists-header">
          <h2>EQ Presets</h2>
          <span className="header-actions">
            <button
              type="button"
              onClick={() => void reloadPresets()}
              title="Re-read every preset YAML from disk and report parse errors"
            >
              Reload
            </button>
            <button
              type="button"
              className="btn-primary"
              onClick={() => setCreating(true)}
            >
              + New
            </button>
          </span>
        </header>
        <p className="muted small">
          Audio effects applied to the music (ambient) channel. Stack a graphic
          EQ and colour effects — they apply top to bottom.
        </p>
        <ul className="playlist-list">
          {presets.length === 0 ? (
            <li className="muted small empty">
              No EQ presets in this mode yet — click <strong>+ New</strong> to
              create one.
            </li>
          ) : (
            presets.map((p) => (
              <li
                key={p.id}
                className={`playlist-list-item ${selectedId === p.id ? "active" : ""}`}
              >
                <button
                  type="button"
                  className="playlist-list-item-meta btn-ghost"
                  onClick={() => setSelectedId(p.id)}
                >
                  <span className="playlist-name">{p.name}</span>
                  <span className="muted small">
                    id: {p.id} · {p.effects.length} effect{p.effects.length === 1 ? "" : "s"}
                  </span>
                </button>
              </li>
            ))
          )}
        </ul>
      </div>

      <div className="two-pane-pane presets-detail-pane">
        {creating ? (
          <PresetForm
            modeId={activeModeId}
            mode="create"
            onClose={() => setCreating(false)}
            onSaved={async (id) => {
              setCreating(false);
              await refresh();
              setSelectedId(id);
            }}
          />
        ) : selected !== null ? (
          <PresetForm
            modeId={activeModeId}
            mode="edit"
            preset={selected}
            onClose={() => undefined}
            onSaved={async () => {
              await refresh();
            }}
            onDeleted={() => {
              setSelectedId(null);
              void refresh();
            }}
          />
        ) : (
          <div className="empty-detail">
            <EmptyState title="No preset selected">
              Pick one from the list, or click <strong>+ New</strong> to build
              a fresh effect chain.
            </EmptyState>
          </div>
        )}
      </div>
    </div>
  );
}

interface FormProps {
  modeId: string;
  mode: "create" | "edit";
  preset?: PresetManifest;
  onClose: () => void;
  onSaved: (id: string) => void | Promise<void>;
  onDeleted?: () => void;
}

function PresetForm({ modeId, mode, preset, onClose, onSaved, onDeleted }: FormProps) {
  const [id, setId] = useState(preset?.id ?? "");
  const [name, setName] = useState(preset?.name ?? "");
  const [description, setDescription] = useState(preset?.description ?? "");
  const [effects, setEffects] = useState<PresetEffect[]>(
    () => preset?.effects.map((e) => ({ ...e })) ?? [],
  );
  // Optional "when active" overrides — checkbox gates each control; null on save
  // when off (= leave that global alone).
  const [volumeOn, setVolumeOn] = useState(preset?.volume != null);
  const [volume, setVolume] = useState(preset?.volume ?? 0.8);
  const [crossfadeOn, setCrossfadeOn] = useState(preset?.crossfade_ms != null);
  const [crossfadeMs, setCrossfadeMs] = useState(preset?.crossfade_ms ?? 2000);
  const [busy, setBusy] = useState(false);

  useEffect(() => {
    if (mode === "edit" && preset) {
      setId(preset.id);
      setName(preset.name);
      setDescription(preset.description ?? "");
      setEffects(preset.effects.map((e) => ({ ...e })));
      setVolumeOn(preset.volume != null);
      setVolume(preset.volume ?? 0.8);
      setCrossfadeOn(preset.crossfade_ms != null);
      setCrossfadeMs(preset.crossfade_ms ?? 2000);
    }
  }, [mode, preset]);

  // Each effect type is a singleton in the chain: clicking its palette chip
  // adds it (with default params, appended to the end) if absent, or removes
  // it if present. A hand-edited YAML with a duplicate still renders both rows
  // (and each is removable via its module); the chip just toggles the first.
  function toggleEffect(type: string) {
    setEffects((es) => {
      const idx = es.findIndex((e) => e.type === type);
      if (idx >= 0) return es.filter((_, i) => i !== idx);
      if (type === "eq") {
        return [...es, { type: "eq", bands: defaultEqBands() } as PresetEffect];
      }
      const ui = EFFECT_UI[type];
      const params = ui ? Object.fromEntries(ui.params.map((p) => [p.key, p.def])) : {};
      return [...es, { type, ...params } as PresetEffect];
    });
  }

  function setEffectParam(idx: number, key: string, value: number) {
    setEffects((es) =>
      es.map((e, i) => (i === idx ? { ...e, [key]: value } : e)),
    );
  }

  function setEffectBands(idx: number, bands: EqBand[]) {
    setEffects((es) => es.map((e, i) => (i === idx ? { ...e, bands } : e)));
  }

  function updateEffect(idx: number, key: string, value: string) {
    setEffects((es) =>
      es.map((e, i) => {
        if (i !== idx) return e;
        const numeric = key !== "type" && /^-?\d+(\.\d+)?$/.test(value);
        return { ...e, [key]: numeric ? Number(value) : value };
      }),
    );
  }

  function removeEffect(idx: number) {
    setEffects((es) => es.filter((_, i) => i !== idx));
  }

  function moveEffect(idx: number, dir: -1 | 1) {
    setEffects((es) => {
      const next = [...es];
      const target = idx + dir;
      if (target < 0 || target >= next.length) return es;
      [next[idx], next[target]] = [next[target], next[idx]];
      return next;
    });
  }

  async function submit(e: FormEvent) {
    e.preventDefault();
    setBusy(true);
    try {
      if (mode === "create") {
        const payload: Parameters<typeof presetsAdminApi.create>[1] = {
          id: id.trim(),
          name: name.trim(),
          effects,
          volume: volumeOn ? volume : null,
          crossfade_ms: crossfadeOn ? crossfadeMs : null,
        };
        const desc = description.trim();
        if (desc) payload.description = desc;
        await presetsAdminApi.create(modeId, payload);
        toast.success("Preset created", id);
      } else {
        const payload: Parameters<typeof presetsAdminApi.update>[2] = {
          name: name.trim(),
          effects,
          volume: volumeOn ? volume : null,
          crossfade_ms: crossfadeOn ? crossfadeMs : null,
        };
        const desc = description.trim();
        if (desc) payload.description = desc;
        await presetsAdminApi.update(modeId, id, payload);
        toast.success("Preset saved", id);
      }
      await onSaved(id);
    } catch (err) {
      toast.error("Save failed", err instanceof Error ? err.message : undefined);
    } finally {
      setBusy(false);
    }
  }

  async function deletePreset() {
    if (!preset) return;
    const ok = await confirmDialog({
      title: `Delete preset "${preset.id}"?`,
      tone: "danger",
      confirmLabel: "Delete",
    });
    if (!ok) return;
    try {
      await presetsAdminApi.delete(modeId, preset.id);
      toast.success("Preset deleted");
      onDeleted?.();
    } catch (e) {
      toast.error("Delete failed", e instanceof Error ? e.message : undefined);
    }
  }

  const activeTypes = new Set(effects.map((e) => e.type));

  return (
    <form onSubmit={submit} className="preset-form">
      <header className="playlist-detail-header">
        <h2>{mode === "create" ? "New preset" : preset?.name}</h2>
        {mode === "edit" ? (
          <div className="playlist-detail-actions">
            <IconButton
              label="Delete preset"
              icon={<TrashIcon />}
              variant="danger"
              onClick={() => void deletePreset()}
            >
              Delete
            </IconButton>
          </div>
        ) : null}
      </header>

      <div className="playlist-meta-fields">
        <label>
          <span className="muted small">ID</span>
          <input
            value={id}
            onChange={(e) => setId(e.target.value)}
            disabled={mode === "edit"}
            pattern="[a-z0-9][a-z0-9_-]*"
            required
          />
        </label>
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

      <section className="preset-overrides">
        <h3>When active</h3>
        <p className="muted small">
          Optional — a preset can also nudge master volume and the crossfade
          time when you switch it on. Untick to leave that alone. If several
          active presets set one, the last turned on wins.
        </p>
        <div className="override-knobs">
          <div className={`override-knob${volumeOn ? "" : " override-off"}`}>
            <label className="override-toggle">
              <input
                type="checkbox"
                checked={volumeOn}
                onChange={(e) => setVolumeOn(e.target.checked)}
              />
              <span className="muted small">Master volume</span>
            </label>
            <Knob
              label="Volume"
              value={volume}
              min={0}
              max={1}
              step={0.01}
              def={0.8}
              format={(v) => `${Math.round(v * 100)}%`}
              onChange={setVolume}
              disabled={!volumeOn}
            />
          </div>
          <div className={`override-knob${crossfadeOn ? "" : " override-off"}`}>
            <label className="override-toggle">
              <input
                type="checkbox"
                checked={crossfadeOn}
                onChange={(e) => setCrossfadeOn(e.target.checked)}
              />
              <span className="muted small">Crossfade</span>
            </label>
            <Knob
              label="Time"
              value={crossfadeMs}
              min={0}
              max={10000}
              step={100}
              def={2000}
              format={(v) => `${(v / 1000).toFixed(1)}s`}
              onChange={setCrossfadeMs}
              disabled={!crossfadeOn}
            />
          </div>
        </div>
      </section>

      <section>
        <h3>Effects</h3>
        <p className="muted small">
          Click an effect to add it; click again to remove. Active effects apply
          top to bottom — reorder with ↑↓.
        </p>
        <div className="effect-palette" role="group" aria-label="Effects">
          {ADDABLE.map((a) => (
            <button
              key={a.type}
              type="button"
              className="effect-chip"
              aria-pressed={activeTypes.has(a.type)}
              title={a.blurb}
              onClick={() => toggleEffect(a.type)}
            >
              {a.label}
            </button>
          ))}
        </div>
        {effects.length === 0 ? (
          <p className="muted small">No effects active — pick some above.</p>
        ) : (
          <ol className="effect-list">
            {effects.map((eff, idx) => {
              const isEq = eff.type === "eq";
              const ui = EFFECT_UI[eff.type];
              const known = isEq || ui != null;
              return (
                <li key={idx} className="effect-row rack-module">
                  <header>
                    <span className="effect-title">
                      <strong>{isEq ? EQ_META.label : ui?.label ?? eff.type}</strong>
                      {known ? (
                        <span className="muted small">{isEq ? EQ_META.blurb : ui?.blurb}</span>
                      ) : (
                        <span className="ref-missing">
                          ⚠ not supported — this effect does nothing
                        </span>
                      )}
                    </span>
                    <div className="effect-row-actions">
                      <IconButton
                        label="Move effect up"
                        icon={<ArrowUpIcon />}
                        onClick={() => moveEffect(idx, -1)}
                        disabled={idx === 0}
                      />
                      <IconButton
                        label="Move effect down"
                        icon={<ArrowDownIcon />}
                        onClick={() => moveEffect(idx, 1)}
                        disabled={idx === effects.length - 1}
                      />
                      <IconButton
                        label="Remove effect"
                        icon={<XIcon />}
                        variant="danger"
                        onClick={() => removeEffect(idx)}
                      />
                    </div>
                  </header>
                  {isEq ? (
                    <GraphicEqModule
                      bands={normalizeEqBands(eff.bands)}
                      onChange={(b) => setEffectBands(idx, b)}
                    />
                  ) : ui ? (
                    <div className="knob-row">
                      {ui.params.map((pc) => {
                        const cur = Number(eff[pc.key]);
                        const val = Number.isFinite(cur) ? cur : pc.def;
                        // Frequency-type params read better as a tall fader; the
                        // rest are knobs (the hardware-rack vocabulary).
                        return pc.scale === "log" ? (
                          <Fader
                            key={pc.key}
                            value={val}
                            min={pc.min}
                            max={pc.max}
                            step={pc.step}
                            scale="log"
                            height={104}
                            label={pc.label}
                            format={pc.format}
                            def={pc.def}
                            onChange={(v) => setEffectParam(idx, pc.key, v)}
                          />
                        ) : (
                          <Knob
                            key={pc.key}
                            value={val}
                            min={pc.min}
                            max={pc.max}
                            step={pc.step}
                            label={pc.label}
                            format={pc.format}
                            def={pc.def}
                            onChange={(v) => setEffectParam(idx, pc.key, v)}
                          />
                        );
                      })}
                    </div>
                  ) : (
                    // Unknown/unsupported effect — keep raw fields so an existing
                    // preset stays editable (and removable).
                    <div className="effect-params">
                      {Object.entries(eff)
                        .filter(([k]) => k !== "type")
                        .map(([k, v]) => (
                          <label key={k}>
                            <span className="muted small">{k}</span>
                            <input
                              value={String(v)}
                              onChange={(e) => updateEffect(idx, k, e.target.value)}
                            />
                          </label>
                        ))}
                    </div>
                  )}
                </li>
              );
            })}
          </ol>
        )}
      </section>

      <div className="modal-actions">
        {mode === "create" ? (
          <button type="button" onClick={onClose} disabled={busy}>
            Cancel
          </button>
        ) : null}
        <button type="submit" className="btn-primary" disabled={busy}>
          {busy ? "Saving…" : mode === "create" ? "Create" : "Save changes"}
        </button>
      </div>
    </form>
  );
}
