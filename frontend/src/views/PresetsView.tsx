import { useCallback, useEffect, useState } from "react";
import type { FormEvent } from "react";

import { confirmDialog } from "@/components/confirmDialog";
import { EmptyState } from "@/components/EmptyState";
import { Fader } from "@/components/Fader";
import { Field } from "@/components/Field";
import { GraphicEqModule } from "@/components/GraphicEqModule";
import { IconButton } from "@/components/IconButton";
import { TrashIcon, XIcon } from "@/components/icons";
import { Knob } from "@/components/Knob";
import { Switch } from "@/components/Switch";
import { modesAdminApi, presetsAdminApi, presetsApi } from "@/core/api";
import type { PresetEffect, PresetManifest } from "@/core/api";
import { defaultEqBands, normalizeEqBands } from "@/core/eq";
import type { EqBand } from "@/core/eq";
import { usePlayerStore } from "@/core/playerStore";
import { uniqueSlug } from "@/core/slugify";
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

// The full effect rack, in canonical (chain) order — EQ first, then the
// colour/space effects. Every one is always shown as a module; the chain
// applies the enabled ones top-to-bottom in this order.
const ADDABLE: { type: string; label: string; blurb: string }[] = [
  { type: "eq", ...EQ_META },
  ...Object.entries(EFFECT_UI).map(([type, ui]) => ({
    type,
    label: ui.label,
    blurb: ui.blurb,
  })),
];
const ADDABLE_TYPES = new Set(ADDABLE.map((a) => a.type));

/** Default param map for an effect type (eq carries `bands`; the rest carry
 *  the flat numeric params from EFFECT_UI). */
function defaultParamsFor(type: string): Record<string, unknown> {
  if (type === "eq") return { bands: defaultEqBands() };
  const ui = EFFECT_UI[type];
  return ui ? Object.fromEntries(ui.params.map((p) => [p.key, p.def])) : {};
}

/** Params for EVERY known effect type (defaults, overlaid with the preset's
 *  stored values for the ones it includes). Lets a module's controls stay
 *  visible + tuned even while bypassed, so toggling off→on restores it. */
function buildEffectState(effects: PresetEffect[]): Record<string, Record<string, unknown>> {
  const state: Record<string, Record<string, unknown>> = {};
  for (const a of ADDABLE) state[a.type] = defaultParamsFor(a.type);
  for (const e of effects) {
    if (!ADDABLE_TYPES.has(e.type)) continue;
    if (e.type === "eq") {
      state.eq = { bands: normalizeEqBands((e as { bands?: EqBand[] }).bands) };
    } else {
      const rest: Record<string, unknown> = { ...(e as Record<string, unknown>) };
      delete rest.type;
      state[e.type] = { ...state[e.type], ...rest };
    }
  }
  return state;
}

const buildActiveTypes = (effects: PresetEffect[]): Set<string> =>
  new Set(effects.filter((e) => ADDABLE_TYPES.has(e.type)).map((e) => e.type));

// Effects whose type isn't in the rack (e.g. a hand-edited `pitch_shift`) —
// preserved + editable + removable so a save doesn't silently drop them.
const buildExtraEffects = (effects: PresetEffect[]): PresetEffect[] =>
  effects.filter((e) => !ADDABLE_TYPES.has(e.type)).map((e) => ({ ...e }));

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
            existingIds={new Set(presets.map((p) => p.id))}
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
            existingIds={new Set(presets.map((p) => p.id))}
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
  /** Existing preset ids in this mode — used to derive a collision-free slug
   *  from the name on create. */
  existingIds: Set<string>;
  onClose: () => void;
  onSaved: (id: string) => void | Promise<void>;
  onDeleted?: () => void;
}

function PresetForm({ modeId, mode, preset, existingIds, onClose, onSaved, onDeleted }: FormProps) {
  const [name, setName] = useState(preset?.name ?? "");
  const [description, setDescription] = useState(preset?.description ?? "");
  // Params for every effect type + the set of enabled ones. Splitting params
  // from on/off means a bypassed module keeps its tuning (toggle off→on
  // restores it) and the chain is always saved in canonical order.
  const [effectState, setEffectState] = useState(() => buildEffectState(preset?.effects ?? []));
  const [activeTypes, setActiveTypes] = useState(() => buildActiveTypes(preset?.effects ?? []));
  const [extraEffects, setExtraEffects] = useState(() => buildExtraEffects(preset?.effects ?? []));
  // Optional "when active" overrides — a toggle gates each control; null on save
  // when off (= leave that global alone).
  const [volumeOn, setVolumeOn] = useState(preset?.volume != null);
  const [volume, setVolume] = useState(preset?.volume ?? 0.8);
  const [crossfadeOn, setCrossfadeOn] = useState(preset?.crossfade_ms != null);
  const [crossfadeMs, setCrossfadeMs] = useState(preset?.crossfade_ms ?? 2000);
  const [busy, setBusy] = useState(false);

  // Id is the on-disk slug — fixed in edit, derived from the name on create
  // (the operator never types it).
  const presetId =
    mode === "edit" ? (preset?.id ?? "") : uniqueSlug(name, existingIds, "preset");

  useEffect(() => {
    if (mode === "edit" && preset) {
      setName(preset.name);
      setDescription(preset.description ?? "");
      setEffectState(buildEffectState(preset.effects));
      setActiveTypes(buildActiveTypes(preset.effects));
      setExtraEffects(buildExtraEffects(preset.effects));
      setVolumeOn(preset.volume != null);
      setVolume(preset.volume ?? 0.8);
      setCrossfadeOn(preset.crossfade_ms != null);
      setCrossfadeMs(preset.crossfade_ms ?? 2000);
    }
  }, [mode, preset]);

  function toggleEffect(type: string) {
    setActiveTypes((s) => {
      const next = new Set(s);
      if (next.has(type)) next.delete(type);
      else next.add(type);
      return next;
    });
  }

  function setParam(type: string, key: string, value: number) {
    setEffectState((s) => ({ ...s, [type]: { ...s[type], [key]: value } }));
  }

  function setBands(type: string, bands: EqBand[]) {
    setEffectState((s) => ({ ...s, [type]: { ...s[type], bands } }));
  }

  function updateExtra(idx: number, key: string, value: string) {
    setExtraEffects((es) =>
      es.map((e, i) => {
        if (i !== idx) return e;
        const numeric = key !== "type" && /^-?\d+(\.\d+)?$/.test(value);
        return { ...e, [key]: numeric ? Number(value) : value };
      }),
    );
  }

  function removeExtra(idx: number) {
    setExtraEffects((es) => es.filter((_, i) => i !== idx));
  }

  // Compose the saved chain: enabled modules in canonical order, then any
  // preserved unsupported effects.
  function composeEffects(): PresetEffect[] {
    const ordered = ADDABLE.filter((a) => activeTypes.has(a.type)).map((a) =>
      a.type === "eq"
        ? ({ type: "eq", bands: effectState.eq.bands as EqBand[] } as PresetEffect)
        : ({ type: a.type, ...effectState[a.type] } as PresetEffect),
    );
    return [...ordered, ...extraEffects];
  }

  async function submit(e: FormEvent) {
    e.preventDefault();
    if (!name.trim()) return;
    setBusy(true);
    const effects = composeEffects();
    try {
      if (mode === "create") {
        const payload: Parameters<typeof presetsAdminApi.create>[1] = {
          id: presetId,
          name: name.trim(),
          effects,
          volume: volumeOn ? volume : null,
          crossfade_ms: crossfadeOn ? crossfadeMs : null,
        };
        const desc = description.trim();
        if (desc) payload.description = desc;
        await presetsAdminApi.create(modeId, payload);
        toast.success("Preset created", presetId);
      } else {
        const payload: Parameters<typeof presetsAdminApi.update>[2] = {
          name: name.trim(),
          effects,
          volume: volumeOn ? volume : null,
          crossfade_ms: crossfadeOn ? crossfadeMs : null,
        };
        const desc = description.trim();
        if (desc) payload.description = desc;
        await presetsAdminApi.update(modeId, presetId, payload);
        toast.success("Preset saved", presetId);
      }
      await onSaved(presetId);
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
        <Field label="Name">
          <input
            type="text"
            value={name}
            onChange={(e) => setName(e.target.value)}
            required
            autoFocus={mode === "create"}
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

      <section>
        <h3 className="section-label">Effects</h3>
        <p className="muted small">
          The full rack — flip a module's switch to enable it. Enabled effects
          (teal) apply top to bottom; bypassed ones (grey) keep their settings.
          "When active" nudges master volume / crossfade when the preset is
          switched on (last one on wins).
        </p>
        <div className="effect-grid">
          {ADDABLE.filter((a) => a.type !== "eq").map((a) => {
            const type = a.type;
            const active = activeTypes.has(type);
            const params = effectState[type] ?? {};
            return (
              <div key={type} className={`effect-cell${active ? "" : " effect-off"}`}>
                <header className="effect-cell-head">
                  <span className="effect-title" title={a.blurb}>
                    <strong>{a.label}</strong>
                    <div>
                      <span className="muted small effect-blurb">{a.blurb}</span>
                    </div>
                  </span>
                  <Switch
                    className="effect-toggle"
                    checked={active}
                    onChange={() => toggleEffect(type)}
                    aria-label={`Enable ${a.label}`}
                  />
                </header>
                <div className="rack-body">
                  <div className="knob-row">
                    {EFFECT_UI[type].params.map((pc) => {
                      const cur = Number(params[pc.key]);
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
                          height={84}
                          label={pc.label}
                          format={pc.format}
                          def={pc.def}
                          disabled={!active}
                          onChange={(v) => setParam(type, pc.key, v)}
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
                          disabled={!active}
                          onChange={(v) => setParam(type, pc.key, v)}
                        />
                      );
                    })}
                  </div>
                </div>
              </div>
            );
          })}
          <div className="effect-cell overrides-cell">
            <header className="effect-cell-head">
              <span className="effect-title">
                <strong>When active</strong>
              </span>
            </header>
            <div className="rack-body">
              <div className="override-knobs">
                <div className={`override-knob${volumeOn ? "" : " override-off"}`}>
                  <Switch
                    checked={volumeOn}
                    onChange={(e) => setVolumeOn(e.target.checked)}
                    aria-label="Enable master-volume override"
                  />
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
                  <Switch
                    checked={crossfadeOn}
                    onChange={(e) => setCrossfadeOn(e.target.checked)}
                    aria-label="Enable crossfade override"
                  />
                  <Knob
                    label="Crossfade"
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
            </div>
          </div>
          {extraEffects.map((eff, idx) => (
            <div key={`extra-${idx}`} className="effect-cell">
              <header className="effect-cell-head">
                <span className="effect-title">
                  <strong>{eff.type}</strong>
                  <span className="ref-missing">⚠ unsupported</span>
                </span>
                <IconButton
                  label="Remove effect"
                  icon={<XIcon />}
                  variant="danger"
                  onClick={() => removeExtra(idx)}
                />
              </header>
              <div className="rack-body">
                <div className="effect-params">
                  {Object.entries(eff)
                    .filter(([k]) => k !== "type")
                    .map(([k, v]) => (
                      <label key={k}>
                        <span className="muted small">{k}</span>
                        <input
                          type="text"
                          value={String(v)}
                          onChange={(e) => updateExtra(idx, k, e.target.value)}
                        />
                      </label>
                    ))}
                </div>
              </div>
            </div>
          ))}
        </div>

        {/* Graphic EQ — full width, taller, sits below the effect grid. */}
        {(() => {
          const eqActive = activeTypes.has("eq");
          return (
            <div
              className={`effect-row rack-module effect-eq${eqActive ? "" : " effect-off"}`}
            >
              <header>
                <span className="effect-title">
                  <strong>{EQ_META.label}</strong>
                  <span className="muted small">{EQ_META.blurb}</span>
                </span>
                <Switch
                  className="effect-toggle"
                  checked={eqActive}
                  onChange={() => toggleEffect("eq")}
                  aria-label="Enable Graphic EQ"
                />
              </header>
              <div className="rack-body">
                <GraphicEqModule
                  bands={normalizeEqBands(effectState.eq.bands as EqBand[])}
                  active={eqActive}
                  onChange={(b) => setBands("eq", b)}
                />
              </div>
            </div>
          );
        })()}
      </section>

      <div className="form-actions">
        {mode === "create" ? (
          <button type="button" onClick={onClose} disabled={busy}>
            Cancel
          </button>
        ) : null}
        <button type="submit" className="btn-primary" disabled={busy || !name.trim()}>
          {busy ? "Saving…" : mode === "create" ? "Create" : "Save changes"}
        </button>
      </div>
    </form>
  );
}
