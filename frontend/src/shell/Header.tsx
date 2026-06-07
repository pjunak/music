import { useCallback, useEffect, useState } from "react";

import { DeviceNameField } from "@/components/DeviceNameField";
import { HelpIcon } from "@/components/icons";
import { modesApi } from "@/core/api";
import { useAuthStore } from "@/core/auth";
import { usePlayerStore } from "@/core/playerStore";
import type { ModeSummary } from "@/core/types";
import { useUiTransient } from "@/core/uiTransient";
import { wsClient } from "@/core/ws";

import { ModeManagerModal } from "./ModeManagerModal";
import { Tabs } from "./Tabs";

export function Header() {
  const wsStatus = usePlayerStore((s) => s.wsStatus);
  const activeModeId = usePlayerStore((s) => s.state?.active_mode_id ?? null);
  const authStatus = useAuthStore((s) => s.status);
  const isGuest = authStatus !== "authenticated";
  // Only offer "Sign in" once we *know* the viewer is anonymous — during the
  // brief "unknown" boot window we show nothing rather than flashing the
  // button at an operator who's already signed in.
  const isAnonymous = authStatus === "anonymous";

  const [modes, setModes] = useState<ModeSummary[]>([]);
  const [modeMgrOpen, setModeMgrOpen] = useState(false);
  const refreshModes = useCallback(() => {
    // /api/modes requires auth; guests will 401 — silently skip.
    if (isGuest) {
      setModes([]);
      return;
    }
    modesApi.list().then(setModes).catch(() => setModes([]));
  }, [isGuest]);
  useEffect(() => {
    refreshModes();
  }, [refreshModes]);

  const openShortcutSheet = useUiTransient((s) => s.setShortcutSheetOpen);
  const setLoginOpen = useUiTransient((s) => s.setLoginOpen);

  function changeMode(modeId: string) {
    wsClient.send({
      type: "set_active_mode",
      mode_id: modeId === "" ? null : modeId,
    });
  }

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
      {/* The authoring tabs are operator-only; hiding the strip keeps guests on
          the TV surface (a protected route they hit directly just shows the
          in-place sign-in gate). */}
      {isGuest ? <span className="tabs-placeholder" /> : <Tabs />}
      <div className="app-header-right">
        {/* Mode picker: lives in the header so it's reachable from any
            tab without jumping to Console. Authed-only because /api/modes
            401s for guests; guests never need to pick a mode anyway since
            they're on the read-only TV view. */}
        {!isGuest ? (
          <div className="header-mode-picker" title="Active mode">
            <span className="muted small">mode</span>
            <select
              value={activeModeId ?? ""}
              onChange={(e) => changeMode(e.target.value)}
              aria-label="Active mode"
            >
              <option value="">— none —</option>
              {modes.map((m) => (
                <option key={m.id} value={m.id}>
                  {m.name}
                </option>
              ))}
            </select>
            <button
              type="button"
              className="btn-ghost header-mode-manage"
              onClick={() => setModeMgrOpen(true)}
              title="Manage modes (create / rename / delete)"
              aria-label="Manage modes"
            >
              ⚙
            </button>
          </div>
        ) : null}
        <button
          type="button"
          className="header-help btn-ghost"
          onClick={() => openShortcutSheet(true)}
          title="Keyboard shortcuts (?)"
          aria-label="Show keyboard shortcuts"
        >
          <HelpIcon />
        </button>
        {isAnonymous ? (
          <button
            type="button"
            className="btn-link guest-signin-link"
            onClick={() => setLoginOpen(true)}
          >
            Sign in
          </button>
        ) : null}
      </div>
      {!isGuest ? (
        <ModeManagerModal
          open={modeMgrOpen}
          onClose={() => setModeMgrOpen(false)}
          modes={modes}
          activeModeId={activeModeId}
          onChanged={refreshModes}
        />
      ) : null}
    </header>
  );
}
