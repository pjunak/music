import { useEffect, useState } from "react";
import { Link } from "react-router-dom";

import { DeviceNameField } from "@/components/DeviceNameField";
import { modesApi } from "@/core/api";
import { useAuthStore } from "@/core/auth";
import { usePlayerStore } from "@/core/playerStore";
import type { ModeSummary } from "@/core/types";

import { Tabs } from "./Tabs";

export function Header() {
  const wsStatus = usePlayerStore((s) => s.wsStatus);
  const activeModeId = usePlayerStore((s) => s.state?.active_mode_id ?? null);
  const activeSceneId = usePlayerStore((s) => s.state?.active_scene_id ?? null);
  const authStatus = useAuthStore((s) => s.status);
  const isGuest = authStatus !== "authenticated";

  const [modes, setModes] = useState<ModeSummary[]>([]);
  useEffect(() => {
    // /api/modes requires auth; guests will 401 — silently skip.
    if (isGuest) {
      setModes([]);
      return;
    }
    modesApi.list().then(setModes).catch(() => setModes([]));
  }, [isGuest]);

  const activeMode = modes.find((m) => m.id === activeModeId) ?? null;

  return (
    <header className="app-header">
      <div className="app-header-left">
        <DeviceNameField />
        <span
          className={`ws-status ws-status-${wsStatus}`}
          title={`WebSocket: ${wsStatus}`}
        >
          <span className="ws-status-dot" aria-hidden="true" />
          <span className="ws-status-text">{wsStatus}</span>
        </span>
      </div>
      {/* Guests see only the Player route. Hiding the tab strip prevents
          confusion where clicking "Library" just bounces them to /login. */}
      {isGuest ? <span className="tabs-placeholder" /> : <Tabs />}
      <div className="app-header-right">
        {activeModeId !== null ? (
          <span className="context-badge" title="Active mode / scene">
            <span className="muted small">mode</span>
            <strong>
              {activeMode?.name ?? activeModeId}
            </strong>
            {activeSceneId !== null ? (
              <>
                <span className="muted small">·</span>
                <span className="muted small">scene</span>
                <strong>{activeSceneId}</strong>
              </>
            ) : null}
          </span>
        ) : null}
        {isGuest ? (
          <Link to="/login" className="btn-ghost guest-signin-link">
            Sign in
          </Link>
        ) : null}
      </div>
    </header>
  );
}
