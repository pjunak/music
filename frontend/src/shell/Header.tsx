import { useEffect, useState } from "react";

import { modesApi } from "@/core/api";
import { usePlayerStore } from "@/core/playerStore";
import type { ModeSummary } from "@/core/types";

import { Tabs } from "./Tabs";

export function Header() {
  const wsStatus = usePlayerStore((s) => s.wsStatus);
  const activeModeId = usePlayerStore((s) => s.state?.active_mode_id ?? null);
  const activeSceneId = usePlayerStore((s) => s.state?.active_scene_id ?? null);

  const [modes, setModes] = useState<ModeSummary[]>([]);
  useEffect(() => {
    modesApi.list().then(setModes).catch(() => setModes([]));
  }, []);

  const activeMode = modes.find((m) => m.id === activeModeId) ?? null;

  return (
    <header className="app-header">
      <div className="app-header-left">
        <h1>Music</h1>
        <span className={`ws-status ws-status-${wsStatus}`}>{wsStatus}</span>
      </div>
      <Tabs />
      <div className="app-header-right">
        {activeModeId !== null ? (
          <span className="context-badge" title="Active mode / scene">
            <span className="muted small">mode</span>
            <strong>{activeMode?.name ?? activeModeId}</strong>
            {activeSceneId !== null ? (
              <>
                <span className="muted small">·</span>
                <span className="muted small">scene</span>
                <strong>{activeSceneId}</strong>
              </>
            ) : null}
          </span>
        ) : null}
      </div>
    </header>
  );
}
