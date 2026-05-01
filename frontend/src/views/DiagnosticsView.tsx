import { useEffect, useState } from "react";

import { diagnosticsApi } from "@/core/api";
import type { DiagnosticsResponse } from "@/core/api";
import { useAuthStore } from "@/core/auth";
import { playbackEngine } from "@/core/playbackEngine";
import { selectIsMyOutput, usePlayerStore } from "@/core/playerStore";

/** Fixed reference for "never". Avoids "Invalid Date" rendering when a
 *  loader / scan hasn't run since boot. */
const NEVER = "(never)";

function formatAgo(unixSeconds: number | null): string {
  if (unixSeconds === null) return NEVER;
  const ageS = Date.now() / 1000 - unixSeconds;
  if (ageS < 60) return `${Math.round(ageS)}s ago`;
  if (ageS < 3600) return `${Math.round(ageS / 60)}m ago`;
  if (ageS < 86400) return `${Math.round(ageS / 3600)}h ago`;
  return `${Math.round(ageS / 86400)}d ago`;
}

/** Full-page diagnostics. Same data as the Settings → Diagnostics card,
 *  but exposed at /diagnostics so the operator can open it in a new tab
 *  (kept open while debugging audio elsewhere). Public route — no auth
 *  required, so it's also reachable from a TV bookmark. */
export function DiagnosticsView() {
  const [, setTick] = useState(0);
  useEffect(() => {
    const t = window.setInterval(() => setTick((n) => n + 1), 500);
    return () => window.clearInterval(t);
  }, []);

  const wsStatus = usePlayerStore((s) => s.wsStatus);
  const myDeviceId = usePlayerStore((s) => s.myDeviceId);
  const isMyOutput = usePlayerStore(selectIsMyOutput);
  const activeOutputs = usePlayerStore(
    (s) => s.state?.active_output_device_ids ?? [],
  );
  const masterVolumeFromState = usePlayerStore((s) => s.state?.volume ?? null);
  const connectedDevices = usePlayerStore(
    (s) => s.state?.connected_devices ?? [],
  );
  const authStatus = useAuthStore((s) => s.status);

  // Server-side snapshot — polled every 5s while the page is open.
  // Auth-gated: guests see the read-only client-state sections only.
  const [serverDx, setServerDx] = useState<DiagnosticsResponse | null>(null);
  useEffect(() => {
    if (authStatus !== "authenticated") return;
    let cancelled = false;
    const fetchDx = () => {
      void diagnosticsApi
        .get()
        .then((r) => {
          if (!cancelled) setServerDx(r);
        })
        .catch(() => {
          /* keep last-good rendering */
        });
    };
    fetchDx();
    const t = window.setInterval(fetchDx, 5000);
    return () => {
      cancelled = true;
      window.clearInterval(t);
    };
  }, [authStatus]);

  const diagnostics = playbackEngine.getDiagnostics();

  function copyToClipboard() {
    const payload = {
      ws: { status: wsStatus, myDeviceId, isMyOutput, activeOutputs },
      connectedDevices,
      masterVolumeFromState,
      engine: playbackEngine.getDiagnostics(),
      ua: navigator.userAgent,
      timestamp: new Date().toISOString(),
    };
    void navigator.clipboard.writeText(JSON.stringify(payload, null, 2));
  }

  return (
    <div className="diagnostics-view">
      <header className="diagnostics-view-header">
        <h1>Diagnostics</h1>
        <button type="button" onClick={copyToClipboard}>
          Copy JSON
        </button>
      </header>

      <section className="surface-card">
        <h3>Connection</h3>
        <ul className="diagnostics-summary">
          <li>
            <span className="muted small">WebSocket</span>
            <strong>{wsStatus}</strong>
          </li>
          <li>
            <span className="muted small">My device id</span>
            <code>{myDeviceId ?? "(none)"}</code>
          </li>
          <li>
            <span className="muted small">Am I an audio output?</span>
            <strong className={isMyOutput ? "ok" : "danger"}>
              {isMyOutput ? "yes" : "no"}
            </strong>
          </li>
          <li>
            <span className="muted small">Active output device ids</span>
            <code>
              {activeOutputs.length === 0 ? "(none)" : activeOutputs.join(", ")}
            </code>
          </li>
        </ul>
      </section>

      {serverDx !== null ? (
        <section className="surface-card">
          <h3>Server</h3>
          <ul className="diagnostics-summary">
            <li>
              <span className="muted small">Indexed tracks</span>
              <strong>{serverDx.track_count}</strong>
            </li>
            <li>
              <span className="muted small">Last full scan</span>
              <strong>{formatAgo(serverDx.last_scan_at)}</strong>
            </li>
            <li>
              <span className="muted small">State revision</span>
              <code>{serverDx.state_revision}</code>
            </li>
            <li>
              <span className="muted small">Connected devices (server)</span>
              <strong>{serverDx.connected_device_count}</strong>
            </li>
            <li>
              <span className="muted small">Modes loaded</span>
              <strong>{serverDx.modes.loaded_ids.length}</strong>
            </li>
            <li>
              <span className="muted small">Mode load errors</span>
              <strong
                className={
                  Object.keys(serverDx.modes.errors).length > 0
                    ? "danger"
                    : "ok"
                }
              >
                {Object.keys(serverDx.modes.errors).length}
              </strong>
            </li>
            <li>
              <span className="muted small">EQ presets loaded</span>
              <strong>{serverDx.presets.loaded_ids.length}</strong>
            </li>
            <li>
              <span className="muted small">Preset load errors</span>
              <strong
                className={
                  Object.keys(serverDx.presets.errors).length > 0
                    ? "danger"
                    : "ok"
                }
              >
                {Object.keys(serverDx.presets.errors).length}
              </strong>
            </li>
          </ul>
          {Object.keys(serverDx.modes.errors).length > 0 ? (
            <details>
              <summary>Mode load errors</summary>
              <ul className="diagnostics-summary">
                {Object.entries(serverDx.modes.errors).map(([id, err]) => (
                  <li key={id}>
                    <span className="muted small">{id}</span>
                    <code className="error small">{err}</code>
                  </li>
                ))}
              </ul>
            </details>
          ) : null}
          {Object.keys(serverDx.presets.errors).length > 0 ? (
            <details>
              <summary>Preset load errors</summary>
              <ul className="diagnostics-summary">
                {Object.entries(serverDx.presets.errors).map(([id, err]) => (
                  <li key={id}>
                    <span className="muted small">{id}</span>
                    <code className="error small">{err}</code>
                  </li>
                ))}
              </ul>
            </details>
          ) : null}
        </section>
      ) : null}

      <section className="surface-card">
        <h3>Connected devices ({connectedDevices.length})</h3>
        {connectedDevices.length === 0 ? (
          <p className="muted small">(none yet)</p>
        ) : (
          <table className="diagnostics-channels">
            <thead>
              <tr>
                <th>Device id</th>
                <th>Name</th>
                <th>Capabilities</th>
                <th>Active output?</th>
                <th>Me?</th>
              </tr>
            </thead>
            <tbody>
              {connectedDevices.map((d) => (
                <tr key={d.device_id}>
                  <td>
                    <code>{d.device_id}</code>
                  </td>
                  <td>{d.name}</td>
                  <td>{d.capabilities.join(", ") || "(none)"}</td>
                  <td>{activeOutputs.includes(d.device_id) ? "yes" : "no"}</td>
                  <td>{d.device_id === myDeviceId ? "yes" : "—"}</td>
                </tr>
              ))}
            </tbody>
          </table>
        )}
      </section>

      <section className="surface-card">
        <h3>Engine state</h3>
        <ul className="diagnostics-summary">
          <li>
            <span className="muted small">Master volume (state)</span>
            <strong>
              {masterVolumeFromState === null
                ? "(no state yet)"
                : `${Math.round(masterVolumeFromState * 100)}%`}
            </strong>
          </li>
          <li>
            <span className="muted small">Master volume (engine)</span>
            <strong>{Math.round(diagnostics.masterVolume * 100)}%</strong>
          </li>
          <li>
            <span className="muted small">is_playing</span>
            <strong>{String(diagnostics.lastIsPlaying)}</strong>
          </li>
          <li>
            <span className="muted small">Current ambient slot</span>
            <strong>{diagnostics.currentSlot}</strong>
          </li>
          <li>
            <span className="muted small">Last ambient track id</span>
            <code>{diagnostics.lastAmbientId ?? "(none)"}</code>
          </li>
          <li>
            <span className="muted small">Last interrupt track id</span>
            <code>{diagnostics.lastInterruptId ?? "(none)"}</code>
          </li>
        </ul>
      </section>

      <section className="surface-card">
        <h3>Channels</h3>
        <table className="diagnostics-channels">
          <thead>
            <tr>
              <th>Channel</th>
              <th>Gain</th>
              <th>Volume</th>
              <th>Paused</th>
              <th>Muted</th>
              <th>Ready</th>
              <th>Net</th>
              <th>Time</th>
              <th>Duration</th>
              <th>Error</th>
              <th>Src</th>
            </tr>
          </thead>
          <tbody>
            {diagnostics.channels.map((c) => (
              <tr key={c.label}>
                <td>{c.label}</td>
                <td>{c.gain.toFixed(2)}</td>
                <td>{c.volume.toFixed(2)}</td>
                <td>{c.paused ? "yes" : "no"}</td>
                <td>{c.muted ? "yes" : "no"}</td>
                <td>{c.readyState}</td>
                <td>{c.networkState}</td>
                <td>
                  {Number.isFinite(c.currentTime) ? c.currentTime.toFixed(1) : "—"}
                </td>
                <td>
                  {Number.isFinite(c.duration) ? c.duration.toFixed(1) : "—"}
                </td>
                <td>{c.errorCode ?? "—"}</td>
                <td className="diagnostics-src" title={c.src}>
                  {c.src.replace(/^.*\/api\//, "/api/") || "(empty)"}
                </td>
              </tr>
            ))}
          </tbody>
        </table>
        <p className="muted small">
          <strong>readyState</strong>: 0 nothing, 1 metadata, 2 current data,
          3 future data, 4 enough data.{" "}
          <strong>networkState</strong>: 0 empty, 1 idle, 2 loading, 3 no
          source. <strong>error</strong>: 1 ABORTED, 2 NETWORK, 3 DECODE, 4
          SRC_NOT_SUPPORTED.
        </p>
      </section>
    </div>
  );
}
