import { useEffect, useState } from "react";
import type { FormEvent } from "react";

import { libraryApi } from "@/core/api";
import type { MetadataUpdate } from "@/core/api";
import type { Track } from "@/core/types";

interface Props {
  track: Track;
  onClose: () => void;
  onSaved: (updated: Track) => void;
}

export function MetadataEditor({ track, onClose, onSaved }: Props) {
  const [title, setTitle] = useState(track.title);
  const [artist, setArtist] = useState(track.artist);
  const [albumArtist, setAlbumArtist] = useState(track.album_artist);
  const [album, setAlbum] = useState(track.album);
  const [trackNo, setTrackNo] = useState<string>(
    track.track_no === null ? "" : String(track.track_no),
  );
  const [year, setYear] = useState<string>(
    track.year === null ? "" : String(track.year),
  );
  const [genre, setGenre] = useState(track.genre);
  const [saving, setSaving] = useState(false);
  const [error, setError] = useState<string | null>(null);

  useEffect(() => {
    function onKey(e: KeyboardEvent) {
      if (e.key === "Escape") onClose();
    }
    window.addEventListener("keydown", onKey);
    return () => window.removeEventListener("keydown", onKey);
  }, [onClose]);

  async function onSubmit(e: FormEvent) {
    e.preventDefault();
    setSaving(true);
    setError(null);
    try {
      const payload: MetadataUpdate = {
        title,
        artist,
        album_artist: albumArtist,
        album,
        track_no: trackNo === "" ? null : Number(trackNo),
        year: year === "" ? null : Number(year),
        genre,
      };
      const updated = await libraryApi.updateMetadata(track.id, payload);
      onSaved(updated);
      onClose();
    } catch (e) {
      setError(e instanceof Error ? e.message : "save failed");
    } finally {
      setSaving(false);
    }
  }

  return (
    <div className="modal-backdrop" onMouseDown={onClose}>
      <div
        className="modal"
        role="dialog"
        aria-label="Edit track metadata"
        onMouseDown={(e) => e.stopPropagation()}
      >
        <header className="modal-header">
          <h2>Edit metadata</h2>
          <button type="button" onClick={onClose} aria-label="Close">
            ✕
          </button>
        </header>
        <form onSubmit={onSubmit} className="metadata-form">
          <p className="muted small metadata-path">{track.path}</p>

          <label>
            <span>Title</span>
            <input value={title} onChange={(e) => setTitle(e.target.value)} />
          </label>
          <label>
            <span>Artist</span>
            <input value={artist} onChange={(e) => setArtist(e.target.value)} />
          </label>
          <label>
            <span>Album artist</span>
            <input
              value={albumArtist}
              onChange={(e) => setAlbumArtist(e.target.value)}
            />
          </label>
          <label>
            <span>Album</span>
            <input value={album} onChange={(e) => setAlbum(e.target.value)} />
          </label>
          <div className="metadata-row">
            <label>
              <span>Track #</span>
              <input
                type="number"
                min={0}
                value={trackNo}
                onChange={(e) => setTrackNo(e.target.value)}
              />
            </label>
            <label>
              <span>Year</span>
              <input
                type="number"
                min={0}
                value={year}
                onChange={(e) => setYear(e.target.value)}
              />
            </label>
          </div>
          <label>
            <span>Genre</span>
            <input value={genre} onChange={(e) => setGenre(e.target.value)} />
          </label>

          {error !== null ? <p className="error small">{error}</p> : null}

          <div className="modal-actions">
            <button type="button" onClick={onClose} disabled={saving}>
              Cancel
            </button>
            <button type="submit" disabled={saving}>
              {saving ? "Saving…" : "Save"}
            </button>
          </div>
        </form>
      </div>
    </div>
  );
}
