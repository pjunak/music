import { useCallback, useEffect, useState } from "react";

import { confirmDialog } from "@/components/confirmDialog";
import { SettingsIcon } from "@/components/icons";
import { inputDialog } from "@/components/inputDialog";
import { Switch } from "@/components/Switch";
import { authApi, devicesApi } from "@/core/api";
import type { ActiveSession } from "@/core/api";
import { useAuthStore } from "@/core/auth";
import { usePlayerArray, usePlayerStore } from "@/core/playerStore";
import { toast } from "@/core/toast";
import type { KnownDevice } from "@/core/types";
import { useUiStore } from "@/core/uiStore";

export function SettingsView() {
  const hidePlayerArt = useUiStore((s) => s.hidePlayerArt);
  const setHidePlayerArt = useUiStore((s) => s.setHidePlayerArt);

  const user = useAuthStore((s) => s.user);
  const logout = useAuthStore((s) => s.logout);

  return (
    <div className="settings-view">
      <section className="surface-card">
        <h3>Display</h3>
        <Switch
          checked={hidePlayerArt}
          onChange={(e) => setHidePlayerArt(e.target.checked)}
          label="Hide cover art on Player tab (blackout)"
        />
        <p className="muted small">
          Useful when this tab is the room display and you don't want the art
          dominating the view.
        </p>
      </section>

      <DevicesPanel />

      <section className="surface-card">
        <h3>Account</h3>
        <p className="muted small">
          Signed in as <strong>{user?.username ?? "(unknown)"}</strong>.
        </p>
        <div>
          <button type="button" className="btn-danger" onClick={() => void logout()}>
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
            className="btn-link"
            role="button"
          >
            <SettingsIcon aria-hidden="true" />
            Open diagnostics
            <span aria-hidden="true" className="btn-link-external">↗</span>
          </a>
        </div>
      </section>
    </div>
  );
}

/** The remembered-devices registry: the operator's manually-curated list of
 *  which devices may act as audio outputs. Output is fully manual — a device
 *  only ever produces audio after being saved here AND marked as an output AND
 *  activated. Connected devices not yet saved are surfaced with an "Add"
 *  action; saved devices persist across reinstalls (server-side file). */
function DevicesPanel() {
  const [saved, setSaved] = useState<KnownDevice[] | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [busy, setBusy] = useState(false);
  const connectedDevices = usePlayerArray((s) => s.state?.connected_devices);
  const myDeviceId = usePlayerStore((s) => s.myDeviceId);

  const refresh = useCallback(async () => {
    try {
      setSaved(await devicesApi.list());
      setError(null);
    } catch (e) {
      setError(e instanceof Error ? e.message : "load failed");
    }
  }, []);

  useEffect(() => {
    void refresh();
  }, [refresh]);

  const savedIds = new Set((saved ?? []).map((d) => d.client_id));
  const connectedIds = new Set(connectedDevices.map((d) => d.client_id));
  // Connected devices the operator hasn't remembered yet.
  const unsaved = connectedDevices.filter((d) => !savedIds.has(d.client_id));

  async function save(clientId: string, name: string, isOutput: boolean) {
    setBusy(true);
    try {
      await devicesApi.save(clientId, { name, is_output: isOutput });
      await refresh();
    } catch (e) {
      toast.error("Update failed", e instanceof Error ? e.message : undefined);
    } finally {
      setBusy(false);
    }
  }

  async function rename(d: KnownDevice) {
    const name = await inputDialog({
      title: "Rename device",
      label: "Device name",
      initial: d.name,
    });
    if (name === null) return; // null = cancelled; required+trim handled by the dialog
    await save(d.client_id, name, d.is_output);
  }

  async function forget(d: KnownDevice) {
    const ok = await confirmDialog({
      title: "Forget device?",
      body: `Remove "${d.name}" from the saved list? It can be added again while it's connected.`,
      tone: "danger",
    });
    if (!ok) return;
    setBusy(true);
    try {
      await devicesApi.remove(d.client_id);
      await refresh();
    } catch (e) {
      toast.error("Remove failed", e instanceof Error ? e.message : undefined);
    } finally {
      setBusy(false);
    }
  }

  return (
    <section className="surface-card">
      <h3>Devices</h3>
      <p className="muted small">
        Remember and name your devices. Marking one{" "}
        <strong>output by default</strong> makes it auto-turn-on as a speaker
        whenever it connects (handy for a fixed TV or speaker box). You don't
        need to save a device to use it — any connected device can be turned on
        ad-hoc from the <strong>Speakers</strong> menu in the footer. This list
        is saved and survives reinstalls.
      </p>

      {error !== null ? <p className="error small">{error}</p> : null}

      {saved === null ? (
        <p className="muted small">Loading…</p>
      ) : saved.length === 0 ? (
        <p className="muted small">
          No saved devices yet. Add a connected device below to remember it.
        </p>
      ) : (
        <ul className="device-list">
          {saved.map((d) => (
            <li key={d.client_id} className="device-row">
              <div className="device-row-main">
                <div className="device-row-name">
                  {d.name || "(unnamed)"}
                  {d.client_id === myDeviceId ? (
                    <span className="badge"> this device</span>
                  ) : null}
                </div>
                <p>
                  {connectedIds.has(d.client_id) ? (
                    <span className="badge badge-ok">connected</span>
                  ) : (
                    <span className="badge">offline</span>
                  )}
                </p>
              </div>
              <Switch
                className="device-output-toggle"
                checked={d.is_output}
                disabled={busy}
                onChange={() => void save(d.client_id, d.name, !d.is_output)}
                label="Output by default"
              />
              <div className="device-row-actions">
                <button type="button" onClick={() => void rename(d)} disabled={busy}>
                  Rename
                </button>
                <button
                  type="button"
                  className="btn-danger"
                  onClick={() => void forget(d)}
                  disabled={busy}
                >
                  Forget
                </button>
              </div>
            </li>
          ))}
        </ul>
      )}

      {unsaved.length > 0 ? (
        <div className="device-unsaved">
          <p className="muted small">Connected, not yet saved:</p>
          <ul className="device-list">
            {unsaved.map((d) => (
              <li key={d.client_id} className="device-row">
                <div className="device-row-main">
                  <div className="device-row-name">
                    {d.name || "(unnamed)"}
                    {d.client_id === myDeviceId ? (
                      <span className="badge"> this device</span>
                    ) : null}
                  </div>
                  <p>
                    <span className="badge badge-ok">connected</span>
                  </p>
                </div>
                <div className="device-row-actions">
                  <button
                    type="button"
                    className="btn-primary"
                    onClick={() => void save(d.client_id, d.name, false)}
                    disabled={busy}
                  >
                    Add to list
                  </button>
                </div>
              </li>
            ))}
          </ul>
        </div>
      ) : null}
    </section>
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
        Download a tar.gz of <code>app.db</code>, <code>modes/</code> (including
        each mode's EQ presets), and the saved <code>devices.json</code> — the
        persistent state worth keeping. Music and SFX libraries aren't included;
        back those up at the filesystem level (rsync / SFTP) since they're large
        and change rarely from the app's perspective.
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
              <div className="entity-row-main">
                <div>
                  <code>{s.token_prefix}…</code>
                  {s.is_current ? (
                    <span className="badge badge-accent"> this device</span>
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
