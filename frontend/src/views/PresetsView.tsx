import { useCallback, useEffect, useState } from "react";
import type { FormEvent } from "react";

import { confirmDialog } from "@/components/ConfirmDialog";
import { presetsAdminApi, presetsApi } from "@/core/api";
import type { PresetEffect, PresetManifest } from "@/core/api";
import { toast } from "@/core/toast";

const SUPPORTED_EFFECTS: { type: string; description: string; defaults: PresetEffect }[] = [
  { type: "lowpass", description: "Cuts highs above frequency", defaults: { type: "lowpass", frequency: 800, q: 0.7 } },
  { type: "highpass", description: "Cuts lows below frequency", defaults: { type: "highpass", frequency: 200, q: 0.7 } },
  { type: "bandpass", description: "Keeps a band around frequency", defaults: { type: "bandpass", frequency: 1000, q: 1.0 } },
  { type: "delay", description: "Echo with feedback + wet mix", defaults: { type: "delay", time: 0.25, feedback: 0.3, wet: 0.4 } },
  { type: "distortion", description: "Soft-clip saturation", defaults: { type: "distortion", amount: 50 } },
  { type: "tremolo", description: "Amplitude wobble (rate Hz, depth 0–1)", defaults: { type: "tremolo", rate: 5, depth: 0.5 } },
  { type: "reverb", description: "Synthesised IR reverb", defaults: { type: "reverb", decay: 2.0, wet: 0.4 } },
  { type: "pitch_shift", description: "[skipped — not yet implemented]", defaults: { type: "pitch_shift", semitones: -2 } },
];

export function PresetsView() {
  const [presets, setPresets] = useState<PresetManifest[]>([]);
  const [selectedId, setSelectedId] = useState<string | null>(null);
  const [creating, setCreating] = useState(false);

  const refresh = useCallback(async () => {
    try {
      const list = await presetsApi.list();
      setPresets(list);
      if (selectedId !== null && !list.some((p) => p.id === selectedId)) {
        setSelectedId(null);
      }
    } catch (e) {
      toast.error("Load failed", e instanceof Error ? e.message : undefined);
    }
  }, [selectedId]);

  useEffect(() => {
    void refresh();
  }, [refresh]);

  const selected = presets.find((p) => p.id === selectedId) ?? null;

  return (
    <div className="modes-view">
      <div className="modes-pane modes-list-pane">
        <header className="playlists-header">
          <h2>Presets</h2>
          <button
            type="button"
            className="btn-primary"
            onClick={() => setCreating(true)}
          >
            + New
          </button>
        </header>
        <p className="muted small">
          Audio effect chains. The frontend audio engine applies them to the
          ambient channel only. <code>pitch_shift</code> is skipped silently —
          Web Audio has no native pitch shifter (deferred).
        </p>
        <ul className="playlist-list">
          {presets.length === 0 ? (
            <li className="muted small empty">No presets yet.</li>
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

      <div className="modes-pane modes-detail-pane">
        {creating ? (
          <PresetForm
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
            <p className="muted">Select a preset, or click <strong>+ New</strong>.</p>
          </div>
        )}
      </div>
    </div>
  );
}

interface FormProps {
  mode: "create" | "edit";
  preset?: PresetManifest;
  onClose: () => void;
  onSaved: (id: string) => void | Promise<void>;
  onDeleted?: () => void;
}

function PresetForm({ mode, preset, onClose, onSaved, onDeleted }: FormProps) {
  const [id, setId] = useState(preset?.id ?? "");
  const [name, setName] = useState(preset?.name ?? "");
  const [description, setDescription] = useState(preset?.description ?? "");
  const [effects, setEffects] = useState<PresetEffect[]>(
    () => preset?.effects.map((e) => ({ ...e })) ?? [],
  );
  const [busy, setBusy] = useState(false);

  useEffect(() => {
    if (mode === "edit" && preset) {
      setId(preset.id);
      setName(preset.name);
      setDescription(preset.description ?? "");
      setEffects(preset.effects.map((e) => ({ ...e })));
    }
  }, [mode, preset]);

  function addEffect(type: string) {
    const def = SUPPORTED_EFFECTS.find((s) => s.type === type)?.defaults ?? { type };
    setEffects((es) => [...es, { ...def }]);
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
        const payload: Parameters<typeof presetsAdminApi.create>[0] = {
          id: id.trim(),
          name: name.trim(),
          effects,
        };
        const desc = description.trim();
        if (desc) payload.description = desc;
        await presetsAdminApi.create(payload);
        toast.success("Preset created", id);
      } else {
        const payload: Parameters<typeof presetsAdminApi.update>[1] = {
          name: name.trim(),
          effects,
        };
        const desc = description.trim();
        if (desc) payload.description = desc;
        await presetsAdminApi.update(id, payload);
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
      await presetsAdminApi.delete(preset.id);
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
            <button
              type="button"
              className="btn-danger"
              onClick={() => void deletePreset()}
            >
              🗑 Delete
            </button>
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

      <section>
        <h3>Effect chain</h3>
        <p className="muted small">
          Effects apply in order — the audio passes through each in sequence.
        </p>
        {effects.length === 0 ? (
          <p className="muted small">No effects yet. Add one below.</p>
        ) : (
          <ol className="effect-list">
            {effects.map((eff, idx) => (
              <li key={idx} className="effect-row">
                <header>
                  <strong>{eff.type}</strong>
                  <div className="effect-row-actions">
                    <button
                      type="button"
                      onClick={() => moveEffect(idx, -1)}
                      disabled={idx === 0}
                      title="Move up"
                    >
                      ↑
                    </button>
                    <button
                      type="button"
                      onClick={() => moveEffect(idx, 1)}
                      disabled={idx === effects.length - 1}
                      title="Move down"
                    >
                      ↓
                    </button>
                    <button
                      type="button"
                      className="btn-danger"
                      onClick={() => removeEffect(idx)}
                      title="Remove"
                    >
                      ✕
                    </button>
                  </div>
                </header>
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
              </li>
            ))}
          </ol>
        )}
        <div className="effect-add">
          <select
            defaultValue=""
            onChange={(e) => {
              if (e.target.value) {
                addEffect(e.target.value);
                e.target.value = "";
              }
            }}
          >
            <option value="" disabled>
              + Add effect…
            </option>
            {SUPPORTED_EFFECTS.map((s) => (
              <option key={s.type} value={s.type}>
                {s.type} — {s.description}
              </option>
            ))}
          </select>
        </div>
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
