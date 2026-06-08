import { useEffect, useState } from "react";

import { modesApi } from "@/core/api";
import { useAuthStore } from "@/core/auth";
import { usePlayerStore } from "@/core/playerStore";
import type { SoundboardManifest } from "@/core/types";

import { Modal } from "./Modal";

/** Single keyboard-shortcut row: keys + what they do. */
interface Shortcut {
  /** Sequence of key glyphs to display. Each rendered as a <kbd>. */
  keys: string[];
  label: string;
}

const GLOBAL: Shortcut[] = [
  { keys: ["Space"], label: "Play / pause" },
  { keys: ["←"], label: "Previous track" },
  { keys: ["→"], label: "Next track" },
  { keys: ["L"], label: "Cycle loop mode (off → track → queue)" },
  { keys: ["/"], label: "Focus library search" },
  { keys: ["Esc"], label: "Close the open modal" },
  { keys: ["?"], label: "Show this sheet" },
];

const TABS: Shortcut[] = [
  { keys: ["1"], label: "Console" },
  { keys: ["2"], label: "Library" },
  { keys: ["3"], label: "Authoring (Playlists / Soundboards / Interrupts / EQ Presets / Cues)" },
  { keys: ["4"], label: "Settings" },
];

/** Sheet listing every keyboard shortcut the operator can hit.
 *
 *  Opens from the Header "?" button or by pressing Shift+/ on the keyboard.
 *  SFX hotkeys come from the active mode's currently-selected soundboard so
 *  the list reflects what's actually live; an empty list shows a hint
 *  pointing at the Soundboard panel. */
export function ShortcutSheet({ onClose }: { onClose: () => void }) {
  const activeModeId = usePlayerStore((s) => s.state?.active_mode_id ?? null);
  const activeSoundboardId = usePlayerStore(
    (s) => s.state?.active_soundboard_id ?? null,
  );
  const isAuthed = useAuthStore((s) => s.status === "authenticated");
  const [soundboard, setSoundboard] = useState<SoundboardManifest | null>(null);

  // Pull the active soundboard if one's set. Skipped for guests since
  // modes/<id> requires auth.
  useEffect(() => {
    if (!isAuthed || activeModeId === null || activeSoundboardId === null) {
      setSoundboard(null);
      return;
    }
    let cancelled = false;
    void modesApi
      .get(activeModeId)
      .then((d) => {
        if (!cancelled) setSoundboard(d.soundboards[activeSoundboardId] ?? null);
      })
      .catch(() => {
        if (!cancelled) setSoundboard(null);
      });
    return () => {
      cancelled = true;
    };
  }, [isAuthed, activeModeId, activeSoundboardId]);

  const sfxShortcuts: Shortcut[] = [];
  if (soundboard !== null) {
    for (const cat of soundboard.categories) {
      for (const item of cat.items) {
        if (item.hotkey) {
          sfxShortcuts.push({ keys: [item.hotkey], label: `${item.name} (${cat.name})` });
        }
      }
    }
  }

  return (
    <Modal
      ariaLabel="Keyboard shortcuts"
      title="Keyboard shortcuts"
      className="shortcut-sheet"
      bodyClassName="shortcut-sheet-body"
      closeButton
      onClose={onClose}
    >
      <ShortcutSection title="Global" shortcuts={GLOBAL} />
      <ShortcutSection title="Tabs" shortcuts={TABS} />
      <section className="shortcut-section">
        <h3 className="section-label">SFX hotkeys</h3>
        {sfxShortcuts.length === 0 ? (
          <p className="muted small">
            {activeModeId === null
              ? "Pick a mode to see SFX hotkeys."
              : activeSoundboardId === null
                ? "Pick a soundboard from the Console tab to see its hotkeys."
                : "Active soundboard has no SFX with hotkeys."}
          </p>
        ) : (
          <dl className="shortcut-list">
            {sfxShortcuts.map((s, idx) => (
              <div key={idx} className="shortcut-row">
                <dt>
                  {s.keys.map((k) => (
                    <kbd key={k} className="kbd">
                      {k}
                    </kbd>
                  ))}
                </dt>
                <dd>{s.label}</dd>
              </div>
            ))}
          </dl>
        )}
      </section>
    </Modal>
  );
}

function ShortcutSection({
  title,
  shortcuts,
}: {
  title: string;
  shortcuts: Shortcut[];
}) {
  return (
    <section className="shortcut-section">
      <h3 className="section-label">{title}</h3>
      <dl className="shortcut-list">
        {shortcuts.map((s, idx) => (
          <div key={idx} className="shortcut-row">
            <dt>
              {s.keys.map((k) => (
                <kbd key={k} className="kbd">
                  {k}
                </kbd>
              ))}
            </dt>
            <dd>{s.label}</dd>
          </div>
        ))}
      </dl>
    </section>
  );
}
