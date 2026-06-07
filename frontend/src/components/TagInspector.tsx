import { useEffect, useMemo, useState } from "react";
import type { ChangeEvent } from "react";

import { libraryApi } from "@/core/api";
import type { MetadataUpdate } from "@/core/api";
import { toast } from "@/core/toast";
import { trackTitle } from "@/core/trackDisplay";
import type { Track } from "@/core/types";

import { TagIcon } from "./icons";

/** The one tag editor — edits the current track selection, 1 or N.
 *
 *  This replaces both the old per-row metadata modal AND the separate Tags
 *  tab's bulk panel (the pattern every serious tagger uses: one panel scoped
 *  by the selection):
 *    - 1 track  → fields show that track's tags; editing + Save writes them.
 *    - N tracks → fields show the shared value, or a "‹various›" placeholder
 *                 where they differ; a field you touch is written to all,
 *                 untouched fields are left per-track.
 *
 *  "Changed = will be written": a field counts as armed when its input differs
 *  from the selection's shared value, so the difference between "set to empty"
 *  and "leave alone" is explicit. One save path (`updateBulkMetadata` handles
 *  1 or N). */

type FieldKey =
  | "display_title"
  | "origin"
  | "title"
  | "artist"
  | "album_artist"
  | "album"
  | "track_no"
  | "year"
  | "genre";

interface FieldDef {
  key: FieldKey;
  label: string;
  numeric?: boolean;
  hint?: string;
}

// DB-only library fields (never written to the file) vs tag-backed fields.
const LIBRARY_FIELDS: FieldDef[] = [
  {
    key: "display_title",
    label: "Nickname",
    hint: "A friendly name shown in lists instead of the file's title. Library-only.",
  },
  {
    key: "origin",
    label: "Origin",
    hint: "Provenance — game / film / album. Library-only.",
  },
];
const TAG_FIELDS: FieldDef[] = [
  { key: "title", label: "Title" },
  { key: "artist", label: "Artist" },
  { key: "album_artist", label: "Album artist" },
  { key: "album", label: "Album" },
  { key: "track_no", label: "Track #", numeric: true },
  { key: "year", label: "Year", numeric: true },
  { key: "genre", label: "Genre" },
];
const ALL_FIELDS = [...LIBRARY_FIELDS, ...TAG_FIELDS];

function fieldStr(t: Track, key: FieldKey): string {
  const v = t[key] as string | number | null | undefined;
  return v === null || v === undefined ? "" : String(v);
}

/** Per field: the value shared by every selected track, or `null` when they
 *  differ ("‹various›"). */
function commonValues(tracks: Track[]): Record<FieldKey, string | null> {
  const out = {} as Record<FieldKey, string | null>;
  for (const f of ALL_FIELDS) {
    if (tracks.length === 0) {
      out[f.key] = "";
      continue;
    }
    const first = fieldStr(tracks[0], f.key);
    out[f.key] = tracks.every((t) => fieldStr(t, f.key) === first) ? first : null;
  }
  return out;
}

function initFrom(common: Record<FieldKey, string | null>): Record<FieldKey, string> {
  const out = {} as Record<FieldKey, string>;
  for (const f of ALL_FIELDS) out[f.key] = common[f.key] ?? "";
  return out;
}

/** Split a stored path into its editable filename stem and its extension.
 *  The extension is held back from editing so a rename can't accidentally
 *  drop the file out of the indexer's audio-extension set. `dot > 0` skips
 *  leading-dot names (no real audio file starts with a dot). */
function splitName(path: string): { stem: string; ext: string } {
  const base = path.split("/").pop() ?? path;
  const dot = base.lastIndexOf(".");
  return dot > 0
    ? { stem: base.slice(0, dot), ext: base.slice(dot) }
    : { stem: base, ext: "" };
}

export function TagInspector({
  selectedTracks,
  onSaved,
}: {
  selectedTracks: Track[];
  onSaved: () => void;
}) {
  const common = useMemo(() => commonValues(selectedTracks), [selectedTracks]);
  const [values, setValues] = useState<Record<FieldKey, string>>(() =>
    initFrom(common),
  );
  const [busy, setBusy] = useState(false);

  // Re-seed the inputs whenever the selection — or the selected tracks' values
  // after a save+refetch — changes. Typing lives in local state only, so it
  // doesn't trip this (the parent doesn't re-render while you type).
  useEffect(() => {
    setValues(initFrom(common));
  }, [common]);

  // Filename rename is single-track only (you can't rename N files to one
  // name). Held separate from the tag fields because it's a different
  // backend op (file move) that can't be batched into the bulk save.
  const filePath = selectedTracks.length === 1 ? selectedTracks[0].path : null;
  const { stem: currentStem, ext } = useMemo(
    () => splitName(filePath ?? ""),
    [filePath],
  );
  const [fileName, setFileName] = useState(currentStem);
  const [renaming, setRenaming] = useState(false);
  useEffect(() => {
    setFileName(currentStem);
  }, [currentStem]);

  const count = selectedTracks.length;

  if (count === 0) {
    return (
      <div className="tag-inspector tag-inspector-empty">
        <h3>
          <TagIcon aria-hidden="true" /> Tags
        </h3>
        <p className="muted small">
          Select a track to view and edit its tags. Select several (tick the
          boxes) to edit them together.
        </p>
      </div>
    );
  }

  const armed = ALL_FIELDS.filter((f) => values[f.key] !== (common[f.key] ?? ""));
  const dirty = armed.length > 0;

  function set(key: FieldKey, e: ChangeEvent<HTMLInputElement>) {
    const v = e.target.value;
    setValues((prev) => ({ ...prev, [key]: v }));
  }

  function revert() {
    setValues(initFrom(common));
  }

  const renameDirty =
    filePath !== null &&
    fileName.trim() !== "" &&
    fileName.trim() !== currentStem;

  async function renameFile() {
    if (filePath === null || !renameDirty) return;
    const next = fileName.trim();
    if (next.includes("/") || next.includes("\\")) {
      toast.error("Invalid filename", "A filename can't contain slashes.");
      return;
    }
    const track = selectedTracks[0];
    const parent = track.path.split("/").slice(0, -1).join("/");
    setRenaming(true);
    try {
      await libraryApi.moveTrack(track.id, parent, `${next}${ext}`);
      toast.success("File renamed", `${next}${ext}`);
      onSaved();
    } catch (e) {
      toast.error("Rename failed", e instanceof Error ? e.message : undefined);
    } finally {
      setRenaming(false);
    }
  }

  async function save() {
    if (!dirty) return;
    const updates: MetadataUpdate = {};
    for (const f of armed) {
      const v = values[f.key];
      switch (f.key) {
        case "track_no":
          updates.track_no = v === "" ? null : Number(v);
          break;
        case "year":
          updates.year = v === "" ? null : Number(v);
          break;
        case "display_title":
          updates.display_title = v;
          break;
        case "origin":
          updates.origin = v;
          break;
        case "title":
          updates.title = v;
          break;
        case "artist":
          updates.artist = v;
          break;
        case "album_artist":
          updates.album_artist = v;
          break;
        case "album":
          updates.album = v;
          break;
        case "genre":
          updates.genre = v;
          break;
      }
    }
    setBusy(true);
    try {
      const ids = selectedTracks.map((t) => t.id);
      const r = await libraryApi.updateBulkMetadata({ track_ids: ids, updates });
      const n = r.updated.length;
      if (r.skipped.length === 0) {
        toast.success(`Updated ${n} track${n === 1 ? "" : "s"}`);
      } else {
        const sample = r.skipped
          .slice(0, 3)
          .map((s) => `#${s.track_id}: ${s.reason}`)
          .join("\n");
        const more =
          r.skipped.length > 3 ? `\n…and ${r.skipped.length - 3} more` : "";
        toast.warn(
          `Updated ${n}, skipped ${r.skipped.length}`,
          `${sample}${more}`,
        );
      }
      onSaved();
    } catch (e) {
      toast.error("Save failed", e instanceof Error ? e.message : undefined);
    } finally {
      setBusy(false);
    }
  }

  const heading = count === 1 ? trackTitle(selectedTracks[0]) : `${count} tracks`;

  const renderField = (f: FieldDef) => {
    const isVarious = common[f.key] === null;
    const changed = values[f.key] !== (common[f.key] ?? "");
    return (
      <label key={f.key} className={`insp-field${changed ? " changed" : ""}`}>
        <span className="insp-field-label">{f.label}</span>
        <input
          type={f.numeric ? "number" : "text"}
          min={f.numeric ? 0 : undefined}
          value={values[f.key]}
          placeholder={isVarious ? "‹various›" : ""}
          onChange={(e) => set(f.key, e)}
        />
        {f.hint ? <span className="insp-field-hint muted small">{f.hint}</span> : null}
      </label>
    );
  };

  return (
    <div className="tag-inspector">
      <h3>
        <TagIcon aria-hidden="true" /> Tags <span className="muted">· {heading}</span>
      </h3>
      <div className="tag-inspector-body">
        {filePath !== null ? (
          <div className="insp-group">
            <div className="insp-group-head">File</div>
            <div className="insp-file">
              <span className="insp-field-label">Filename</span>
              <div className="insp-file-row">
                <input
                  className={`insp-file-input${renameDirty ? " changed" : ""}`}
                  type="text"
                  value={fileName}
                  spellCheck={false}
                  onChange={(e) => setFileName(e.target.value)}
                  onKeyDown={(e) => {
                    if (e.key === "Enter") void renameFile();
                  }}
                />
                {ext ? <span className="insp-file-ext">{ext}</span> : null}
                <button
                  type="button"
                  className="btn-primary"
                  disabled={renaming || !renameDirty}
                  onClick={() => void renameFile()}
                >
                  {renaming ? "Renaming…" : "Rename"}
                </button>
              </div>
              <span className="insp-field-hint muted small">
                Renames the file on disk in place. The extension is kept.
              </span>
            </div>
          </div>
        ) : null}
        <div className="insp-group">
          <div className="insp-group-head">Library only</div>
          {LIBRARY_FIELDS.map(renderField)}
        </div>
        <div className="insp-group">
          <div className="insp-group-head">Written to the file</div>
          {TAG_FIELDS.map(renderField)}
        </div>
      </div>
      <div className="tag-inspector-foot">
        <button
          type="button"
          className="btn-primary"
          disabled={!dirty || busy}
          onClick={() => void save()}
        >
          {busy
            ? "Saving…"
            : count === 1
              ? "Save"
              : `Save to ${count}`}
        </button>
        {dirty ? (
          <button type="button" className="btn-ghost" onClick={revert} disabled={busy}>
            Revert
          </button>
        ) : null}
        {dirty ? (
          <span className="muted small">
            {armed.length} field{armed.length === 1 ? "" : "s"} changed
          </span>
        ) : null}
      </div>
    </div>
  );
}
