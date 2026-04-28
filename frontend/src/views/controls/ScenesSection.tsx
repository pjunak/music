import { useEffect, useState } from "react";

import { modesApi } from "@/core/api";
import { usePlayerStore } from "@/core/playerStore";
import type { ModeDetail } from "@/core/types";
import { wsClient } from "@/core/ws";

export function ScenesSection() {
  const activeModeId = usePlayerStore((s) => s.state?.active_mode_id ?? null);
  const activeSceneId = usePlayerStore((s) => s.state?.active_scene_id ?? null);

  const [mode, setMode] = useState<ModeDetail | null>(null);
  const [error, setError] = useState<string | null>(null);

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
    return <p className="muted small">Pick a mode to use scenes.</p>;
  }
  if (error !== null) return <p className="error small">{error}</p>;
  if (mode === null) return <p className="muted small">Loading…</p>;

  const scenes = Object.values(mode.scenes);
  if (scenes.length === 0) {
    return (
      <p className="muted small">
        Mode <code>{activeModeId}</code> has no scenes defined.
      </p>
    );
  }

  function activate(id: string) {
    wsClient.send({ type: "activate_scene", scene_id: id });
  }
  function deactivate() {
    wsClient.send({ type: "deactivate_scene" });
  }

  return (
    <div className="scenes-section">
      {activeSceneId !== null ? (
        <div className="scene-active-bar">
          <span>
            <strong>Active:</strong> {mode.scenes[activeSceneId]?.name ?? activeSceneId}
          </span>
          <button type="button" onClick={deactivate}>
            Deactivate
          </button>
        </div>
      ) : null}
      <div className="scenes-grid">
        {scenes.map((s) => {
          const isActive = activeSceneId === s.id;
          return (
            <button
              key={s.id}
              type="button"
              className={`scene-button${isActive ? " active" : ""}`}
              onClick={() => activate(s.id)}
              title={s.description ?? undefined}
            >
              <span className="scene-name">{s.name}</span>
              {s.description ? (
                <span className="muted small">{s.description}</span>
              ) : null}
            </button>
          );
        })}
      </div>
    </div>
  );
}
