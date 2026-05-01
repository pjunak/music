import { useAuthStore } from "@/core/auth";
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
