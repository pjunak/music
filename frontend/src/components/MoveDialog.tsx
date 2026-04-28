import { useEffect, useState } from "react";
import type { FormEvent } from "react";

import { libraryApi } from "@/core/api";
import type { Track } from "@/core/types";

interface Props {
  track: Track;
  /** All folder paths currently known under MUSIC_DIR — pre-populates the
   *  destination dropdown so the user doesn't have to type. */
  knownFolders: string[];
  onClose: () => void;
  onMoved: (updated: Track) => void;
}

function parentOf(path: string): string {
  const idx = path.lastIndexOf("/");
  return idx >= 0 ? path.slice(0, idx) : "";
}

function basenameOf(path: string): string {
  const idx = path.lastIndexOf("/");
  return idx >= 0 ? path.slice(idx + 1) : path;
}

export function MoveDialog({ track, knownFolders, onClose, onMoved }: Props) {
  const [destination, setDestination] = useState(parentOf(track.path));
  const [filename, setFilename] = useState(basenameOf(track.path));
  const [saving, setSaving] = useState(false);
  const [error, setError] = useState<string | null>(null);

  useEffect(() => {
    function onKey(e: KeyboardEvent) {
      if (e.key === "Escape") onClose();
    }
    window.addEventListener("keydown", onKey);
    return () => window.removeEventListener("keydown", onKey);
  }, [onClose]);

  const folderOptions = ["", ...knownFolders.filter((f) => f !== "")].sort();

  async function onSubmit(e: FormEvent) {
    e.preventDefault();
    setSaving(true);
    setError(null);
    try {
      const updated = await libraryApi.moveTrack(track.id, destination, filename);
      onMoved(updated);
      onClose();
    } catch (e) {
      setError(e instanceof Error ? e.message : "move failed");
    } finally {
      setSaving(false);
    }
  }

  return (
    <div className="modal-backdrop" onMouseDown={onClose}>
      <div
        className="modal"
        role="dialog"
        aria-label="Move or rename track"
        onMouseDown={(e) => e.stopPropagation()}
      >
        <header className="modal-header">
          <h2>Move / rename</h2>
          <button type="button" onClick={onClose} aria-label="Close">
            ✕
          </button>
        </header>
        <form onSubmit={onSubmit} className="metadata-form">
          <p className="muted small metadata-path">From: {track.path}</p>

          <label>
            <span>Destination folder</span>
            <input
              list="folder-options"
              value={destination}
              onChange={(e) => setDestination(e.target.value)}
              placeholder="(root)"
            />
            <datalist id="folder-options">
              {folderOptions.map((f) => (
                <option key={f || "(root)"} value={f} />
              ))}
            </datalist>
          </label>
          <label>
            <span>Filename</span>
            <input
              value={filename}
              onChange={(e) => setFilename(e.target.value)}
              required
            />
          </label>

          {error !== null ? <p className="error small">{error}</p> : null}

          <div className="modal-actions">
            <button type="button" onClick={onClose} disabled={saving}>
              Cancel
            </button>
            <button type="submit" disabled={saving}>
              {saving ? "Moving…" : "Move"}
            </button>
          </div>
        </form>
      </div>
    </div>
  );
}
