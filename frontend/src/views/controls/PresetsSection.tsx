import { useEffect, useState } from "react";

import { EmptyState } from "@/components/EmptyState";
import { presetsApi } from "@/core/api";
import type { PresetManifest } from "@/core/api";
import { usePlayerArray } from "@/core/playerStore";
import { wsClient } from "@/core/ws";

export function PresetsSection() {
  // Stable selector via the helper — never mints a fresh [] when state is null
  // (which would loop useSyncExternalStore to React #185 on a cold load).
  const activeIds = usePlayerArray((s) => s.state?.active_preset_ids);
  const [presets, setPresets] = useState<PresetManifest[]>([]);
  const [error, setError] = useState<string | null>(null);

  useEffect(() => {
    let cancelled = false;
    void presetsApi
      .list()
      .then((p) => {
        if (!cancelled) setPresets(p);
      })
      .catch((e: unknown) => {
        if (!cancelled)
          setError(e instanceof Error ? e.message : "failed to load presets");
      });
    return () => {
      cancelled = true;
    };
  }, []);

  function toggle(id: string) {
    const next = activeIds.includes(id)
      ? activeIds.filter((p) => p !== id)
      : [...activeIds, id];
    wsClient.send({ type: "set_active_presets", preset_ids: next });
  }

  function clear() {
    wsClient.send({ type: "set_active_presets", preset_ids: [] });
  }

  if (error !== null) return <p className="error small">{error}</p>;
  if (presets.length === 0) {
    return (
      <EmptyState>
        No presets installed. Add one from <strong>Authoring → Presets</strong>.
      </EmptyState>
    );
  }

  return (
    <div className="presets-section">
      <div className="presets-grid">
        {presets.map((p) => {
          const on = activeIds.includes(p.id);
          return (
            <button
              key={p.id}
              type="button"
              className={`preset-chip${on ? " active" : ""}`}
              onClick={() => toggle(p.id)}
              title={p.description ?? undefined}
            >
              {p.name}
            </button>
          );
        })}
      </div>
      {activeIds.length > 0 ? (
        <button type="button" className="preset-clear" onClick={clear}>
          Clear all
        </button>
      ) : null}
    </div>
  );
}
