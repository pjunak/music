import { useEffect, useState } from "react";

import { playbackEngine } from "@/core/playbackEngine";
import { selectIsMyOutput, usePlayerStore } from "@/core/playerStore";

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
