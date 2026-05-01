import { useCallback, useEffect, useState } from "react";

import { confirmDialog } from "@/components/confirmDialog";
import { authApi } from "@/core/api";
import type { ActiveSession } from "@/core/api";
import { useAuthStore } from "@/core/auth";
import { toast } from "@/core/toast";
import { useUiStore } from "@/core/uiStore";
import type { Capability } from "@/core/uiStore";
import { wsClient } from "@/core/ws";

const ALL_CAPS: { key: Capability; label: string; description: string }[] = [
  {
    key: "controls",
    label: "Controller",
    description:
      "This tab can send actions (play, pause, fire SFX, activate scenes).",
  },
  {
    key: "audio_output",
    label: "Audio output",
    description:
      "This tab actually plays audio. Enable on devices connected to speakers; disable on a phone you only use as a remote.",
  },
];

export function SettingsView() {
  const hidePlayerArt = useUiStore((s) => s.hidePlayerArt);
  const setHidePlayerArt = useUiStore((s) => s.setHidePlayerArt);

  const capabilities = useUiStore((s) => s.capabilities);
  const setCapabilities = useUiStore((s) => s.setCapabilities);

  const user = useAuthStore((s) => s.user);
  const logout = useAuthStore((s) => s.logout);

  function toggleCap(cap: Capability) {
    const next = capabilities.includes(cap)
      ? capabilities.filter((c) => c !== cap)
      : [...capabilities, cap];
    setCapabilities(next);
    wsClient.sendRegister();
  }

  return (
    <div className="settings-view">
      <section className="surface-card">
        <h3>Display</h3>
        <label className="autotag-toggle">
          <input
            type="checkbox"
            checked={hidePlayerArt}
            onChange={(e) => setHidePlayerArt(e.target.checked)}
          />
          <span>Hide cover art on Player tab (blackout)</span>
        </label>
        <p className="muted small">
          Useful when this tab is the room display and you don't want the art
          dominating the view.
        </p>
      </section>

      <section className="surface-card">
        <h3>This device</h3>
        <p className="muted small">
          Rename this device on the <strong>Player</strong> tab — the field
          there is reachable to guest sessions too.
        </p>

        <div className="settings-caps">
          {ALL_CAPS.map((cap) => {
            const on = capabilities.includes(cap.key);
            return (
              <label key={cap.key} className="settings-cap">
                <input
                  type="checkbox"
                  checked={on}
                  onChange={() => toggleCap(cap.key)}
                />
                <div>
                  <div className="settings-cap-label">{cap.label}</div>
                  <p className="muted small">{cap.description}</p>
                </div>
              </label>
            );
          })}
        </div>
      </section>

      <section className="surface-card">
        <h3>Account</h3>
        <p className="muted small">
          Signed in as <strong>{user?.username ?? "(unknown)"}</strong>.
        </p>
        <div>
          <button type="button" onClick={() => void logout()}>
            Sign out
          </button>
        </div>
      </section>

      <ActiveSessionsPanel />

      <BackupPanel />

      <section className="surface-card">
        <h3>Diagnostics</h3>
        <p className="muted small">
          For debugging "no audio" or "device not showing up" issues. Opens in
          a new tab so you can keep it open while clicking around in the main
          window.
        </p>
        <div>
          <a
            href="/diagnostics"
            target="_blank"
            rel="noopener noreferrer"
          >
            🔧 Open diagnostics in new tab
          </a>
        </div>
      </section>
    </div>
  );
}

/** Trigger a backup download. Streams to a Blob (so we can show progress
 *  if the file ever gets big) and offers it via an anchor download. The
 *  backup payload is currently small (DB + YAML), so a Blob is fine; if
 *  the operator's modes/presets ever grow huge, swap to direct navigation. */
function BackupPanel() {
  const [busy, setBusy] = useState(false);

  async function download() {
    setBusy(true);
    try {
      const response = await fetch("/api/admin/backup", {
        credentials: "include",
      });
      if (!response.ok) {
        const text = await response.text().catch(() => "");
        throw new Error(text || `HTTP ${response.status}`);
      }
      const disposition = response.headers.get("content-disposition") ?? "";
      const match = disposition.match(/filename="([^"]+)"/);
      const fname = match ? match[1] : "music-backup.tar.gz";
      const blob = await response.blob();
      const url = URL.createObjectURL(blob);
      const a = document.createElement("a");
      a.href = url;
      a.download = fname;
      document.body.appendChild(a);
      a.click();
      a.remove();
      URL.revokeObjectURL(url);
      toast.success("Backup downloaded");
    } catch (e) {
      toast.error("Backup failed", e instanceof Error ? e.message : undefined);
    } finally {
      setBusy(false);
    }
  }

  return (
    <section className="surface-card">
      <h3>Backup</h3>
      <p className="muted small">
        Download a tar.gz of <code>app.db</code>, <code>modes/</code>, and{" "}
        <code>presets/</code> — the persistent state worth keeping. Music and
        SFX libraries aren't included; back those up at the filesystem level
        (rsync / SFTP) since they're large and change rarely from the app's
        perspective.
      </p>
      <p className="muted small">
        Restore is manual: stop the server, replace the files in place, start
        again.
      </p>
      <div>
        <button
          type="button"
          className="btn-primary"
          onClick={() => void download()}
          disabled={busy}
        >
          {busy ? "Building backup…" : "Download backup"}
        </button>
      </div>
    </section>
  );
}

function formatTime(iso: string): string {
  try {
    return new Date(iso).toLocaleString();
  } catch {
    return iso;
  }
}

/** Lists active auth sessions for the current user with a per-row "revoke"
 *  button. The current session is marked and not revokable here (use Sign
 *  out instead — that's the explicit single-session-vs-all distinction).
 *  Useful when a forgotten browser tab on a TV is sitting on a stale
 *  session and the operator wants to evict it without nuking their own. */
function ActiveSessionsPanel() {
  const [sessions, setSessions] = useState<ActiveSession[] | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [busy, setBusy] = useState<string | null>(null);

  const refresh = useCallback(async () => {
    try {
      const rows = await authApi.listSessions();
      setSessions(rows);
      setError(null);
    } catch (e) {
      setError(e instanceof Error ? e.message : "load failed");
    }
  }, []);

  useEffect(() => {
    void refresh();
  }, [refresh]);

  async function revoke(s: ActiveSession) {
    const ok = await confirmDialog({
      title: "Revoke session?",
      body: `Sign out the session that started ${formatTime(s.created_at)}?`,
      tone: "danger",
    });
    if (!ok) return;
    setBusy(s.token_prefix);
    try {
      await authApi.revokeSession(s.token_prefix);
      toast.success("Session revoked");
      await refresh();
    } catch (e) {
      toast.error(
        "Revoke failed",
        e instanceof Error ? e.message : undefined,
      );
    } finally {
      setBusy(null);
    }
  }

  return (
    <section className="surface-card">
      <h3>Active sessions</h3>
      <p className="muted small">
        Every browser or TV tab signed in to this account. Sign out an
        individual session to evict a forgotten tab without disturbing the
        others.
      </p>
      {error !== null ? (
        <p className="error small">{error}</p>
      ) : sessions === null ? (
        <p className="muted small">Loading…</p>
      ) : sessions.length === 0 ? (
        <p className="muted small">No active sessions.</p>
      ) : (
        <ul className="simple-list">
          {sessions.map((s) => (
            <li key={s.token_prefix}>
              <div>
                <div>
                  <code>{s.token_prefix}…</code>
                  {s.is_current ? (
                    <span className="badge"> this device</span>
                  ) : null}
                </div>
                <p className="muted small">
                  Last seen {formatTime(s.last_seen)} · started{" "}
                  {formatTime(s.created_at)} · expires{" "}
                  {formatTime(s.expires_at)}
                </p>
              </div>
              <button
                type="button"
                className="btn-danger"
                disabled={s.is_current || busy === s.token_prefix}
                onClick={() => void revoke(s)}
                title={
                  s.is_current
                    ? "Use Sign out above to end this session"
                    : "Revoke this session"
                }
              >
                {busy === s.token_prefix ? "…" : "Revoke"}
              </button>
            </li>
          ))}
        </ul>
      )}
    </section>
  );
}
