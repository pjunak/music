import { useEffect, useState } from "react";

import { EmptyState } from "@/components/EmptyState";
import { VolumeControl } from "@/components/VolumeControl";
import { modesApi } from "@/core/api";
import { usePlayerStore } from "@/core/playerStore";
import type { ModeDetail } from "@/core/types";
import { useUiStore } from "@/core/uiStore";
import { wsClient } from "@/core/ws";

export function SoundboardSection() {
  const activeModeId = usePlayerStore((s) => s.state?.active_mode_id ?? null);
  const activeSoundboardId = usePlayerStore(
    (s) => s.state?.active_soundboard_id ?? null,
  );

  const [mode, setMode] = useState<ModeDetail | null>(null);
  const [error, setError] = useState<string | null>(null);
  const sfxVolume = useUiStore((s) => s.sfxVolume);
  const setSfxVolume = useUiStore((s) => s.setSfxVolume);

  useEffect(() => {
    setError(null);
    if (activeModeId === null) {
      setMode(null);
      return;
    }
    let cancelled = false;
    void modesApi
      .get(activeModeId)
      .then((m) => {
        if (!cancelled) setMode(m);
      })
      .catch((e: unknown) => {
        if (!cancelled) {
          setMode(null);
          setError(e instanceof Error ? e.message : "failed to load mode");
        }
      });
    return () => {
      cancelled = true;
    };
  }, [activeModeId]);

  if (activeModeId === null) {
    return (
      <EmptyState>
        Pick a mode first — soundboards are defined per mode.
      </EmptyState>
    );
  }
  if (error !== null) return <p className="error small">{error}</p>;
  if (mode === null) return <p className="muted small">Loading…</p>;

  const soundboards = Object.values(mode.soundboards);
  if (soundboards.length === 0) {
    return (
      <EmptyState>
        Mode <code>{activeModeId}</code> has no soundboards — add one from the
        Modes tab.
      </EmptyState>
    );
  }

  function pickSoundboard(id: string | null) {
    wsClient.send({ type: "set_active_soundboard", soundboard_id: id });
  }

  function fire(soundboardId: string, itemPath: string) {
    wsClient.send({
      type: "fire_sfx",
      soundboard_id: soundboardId,
      item_path: itemPath,
      volume: sfxVolume,
    });
  }

  const active = activeSoundboardId !== null ? mode.soundboards[activeSoundboardId] : null;

  return (
    <div className="soundboard-section">
      <div className="soundboard-toolbar">
        <label className="mode-picker">
          <span className="muted small">Active soundboard</span>
          <select
            value={activeSoundboardId ?? ""}
            onChange={(e) => pickSoundboard(e.target.value === "" ? null : e.target.value)}
          >
            <option value="">— none —</option>
            {soundboards.map((sb) => (
              <option key={sb.id} value={sb.id}>
                {sb.name || sb.id}
              </option>
            ))}
          </select>
        </label>
        <VolumeControl
          value={sfxVolume}
          onChange={setSfxVolume}
          label="SFX volume"
          showIcon={false}
          prefix="SFX volume"
        />
      </div>

      {active === null ? (
        <p className="muted small">Select a soundboard above to see its items.</p>
      ) : active.categories.length === 0 ? (
        <p className="muted small">Soundboard <code>{active.id}</code> has no items.</p>
      ) : (
        <div className="soundboard-categories">
          {active.categories.map((cat) => (
            <div key={cat.id} className="soundboard-category">
              <h4>{cat.name}</h4>
              <div className="soundboard-grid">
                {cat.items.map((item) => (
                  <button
                    key={item.file}
                    type="button"
                    className="sfx-button"
                    onClick={() => fire(active.id, item.file)}
                    title={item.hotkey ? `Press ${item.hotkey} from anywhere` : item.name}
                  >
                    <span className="sfx-button-name">{item.name}</span>
                    {item.hotkey ? <kbd className="kbd">{item.hotkey}</kbd> : null}
                  </button>
                ))}
              </div>
            </div>
          ))}
        </div>
      )}
    </div>
  );
}
