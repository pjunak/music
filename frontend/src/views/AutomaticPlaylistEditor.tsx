import { useEffect, useMemo, useState } from "react";

import { confirmDialog } from "@/components/confirmDialog";
import { Field } from "@/components/Field";
import {
  type AutomaticPlaylistPreview,
  type AutomaticPlaylistRule,
  type PlaylistMeta,
} from "@/core/types";
import { playlistsApi } from "@/core/api";
import { toast } from "@/core/toast";

function errorMessage(error: unknown): string {
  return error instanceof Error ? error.message : "The request failed unexpectedly.";
}

function tagList(value: string): string[] {
  return value
    .split(",")
    .map((tag) => tag.trim())
    .filter(Boolean);
}

function optionalNumber(value: string): number | null {
  if (!value.trim()) return null;
  const parsed = Number(value);
  return Number.isFinite(parsed) ? parsed : null;
}

interface Props {
  playlist: PlaylistMeta;
  onChanged: () => Promise<void>;
  onTracksChanged: () => Promise<void>;
}

export function AutomaticPlaylistEditor({
  playlist,
  onChanged,
  onTracksChanged,
}: Props) {
  const stored = playlist.automatic_rule;
  const [editing, setEditing] = useState(false);
  const [includeTags, setIncludeTags] = useState("");
  const [match, setMatch] = useState<"any" | "all">("any");
  const [excludeTags, setExcludeTags] = useState("");
  const [tagSources, setTagSources] = useState<"manual" | "manual_and_local">(
    "manual",
  );
  const [minBpm, setMinBpm] = useState("");
  const [maxBpm, setMaxBpm] = useState("");
  const [includeUnknownBpm, setIncludeUnknownBpm] = useState(true);
  const [maximumTracks, setMaximumTracks] = useState(200);
  const [orderBy, setOrderBy] = useState<AutomaticPlaylistRule["order_by"]>(
    "title",
  );
  const [preview, setPreview] = useState<AutomaticPlaylistPreview | null>(null);
  const [previewing, setPreviewing] = useState(false);
  const [saving, setSaving] = useState(false);
  const [refreshing, setRefreshing] = useState(false);

  useEffect(() => {
    setIncludeTags(stored?.include_tags.join(", ") ?? "");
    setMatch(stored?.match ?? "any");
    setExcludeTags(stored?.exclude_tags.join(", ") ?? "");
    setTagSources(stored?.tag_sources ?? "manual");
    setMinBpm(stored?.min_bpm?.toString() ?? "");
    setMaxBpm(stored?.max_bpm?.toString() ?? "");
    setIncludeUnknownBpm(stored?.include_unknown_bpm ?? true);
    setMaximumTracks(stored?.maximum_tracks ?? 200);
    setOrderBy(stored?.order_by ?? "title");
    setPreview(null);
  }, [playlist.id, stored]);

  const rule = useMemo<AutomaticPlaylistRule>(
    () => ({
      schema: "automatic-playlist/v1",
      include_tags: tagList(includeTags),
      match,
      exclude_tags: tagList(excludeTags),
      tag_sources: tagSources,
      min_bpm: optionalNumber(minBpm),
      max_bpm: optionalNumber(maxBpm),
      include_unknown_bpm: includeUnknownBpm,
      maximum_tracks: Math.min(1000, Math.max(1, maximumTracks || 1)),
      order_by: orderBy,
    }),
    [
      excludeTags,
      includeTags,
      includeUnknownBpm,
      match,
      maximumTracks,
      maxBpm,
      minBpm,
      orderBy,
      tagSources,
    ],
  );

  useEffect(() => setPreview(null), [rule]);

  async function previewRule() {
    setPreviewing(true);
    try {
      setPreview(await playlistsApi.previewAutomatic(playlist.id, rule));
    } catch (error) {
      toast.error("Rule preview failed", errorMessage(error));
    } finally {
      setPreviewing(false);
    }
  }

  async function saveRule() {
    if (preview === null) return;
    setSaving(true);
    try {
      const result = await playlistsApi.configureAutomatic(
        playlist.id,
        rule,
        preview.source_signature,
      );
      toast.success(
        "Automatic playlist enabled",
        `${result.materialized_tracks} matching tracks are ready.`,
      );
      setEditing(false);
      await Promise.all([onChanged(), onTracksChanged()]);
    } catch (error) {
      toast.error("Automatic rule not saved", errorMessage(error));
      setPreview(null);
    } finally {
      setSaving(false);
    }
  }

  async function refreshNow() {
    setRefreshing(true);
    try {
      const result = await playlistsApi.refreshAutomatic(playlist.id);
      toast.success("Playlist refreshed", `${result.materialized_tracks} tracks match.`);
      await Promise.all([onChanged(), onTracksChanged()]);
    } catch (error) {
      toast.error("Refresh failed", errorMessage(error));
    } finally {
      setRefreshing(false);
    }
  }

  async function makeManual() {
    const confirmed = await confirmDialog({
      title: `Make "${playlist.name}" manual?`,
      body: (
        "The current resolved tracks will stay in the playlist, but future tag or " +
        "library changes will no longer update it."
      ),
      confirmLabel: "Make manual",
    });
    if (!confirmed) return;
    try {
      await playlistsApi.disableAutomatic(playlist.id);
      toast.success("Playlist is manual", "The current track list was kept.");
      setEditing(false);
      await Promise.all([onChanged(), onTracksChanged()]);
    } catch (error) {
      toast.error("Could not disable automatic mode", errorMessage(error));
    }
  }

  if (!playlist.automatic && !editing) {
    return (
      <section className="playlist-automatic-editor authoring-card">
        <div className="playlist-automatic-heading">
          <div>
            <p className="assistant-eyebrow">Optional automation</p>
            <h3>Keep this playlist current from local tags</h3>
          </div>
          <button type="button" onClick={() => setEditing(true)}>
            Set up automatic rule
          </button>
        </div>
        <p className="muted small">
          Preview a local tag and BPM rule before enabling it. The playlist remains a
          normal manually edited list until you explicitly save that rule.
        </p>
      </section>
    );
  }

  if (playlist.automatic && !editing && stored !== null) {
    return (
      <section className="playlist-automatic-editor authoring-card">
        <div className="playlist-automatic-heading">
          <div>
            <p className="assistant-eyebrow">Automatic playlist</p>
            <h3>Local rule is active</h3>
          </div>
          <span>automatic-playlist/v1</span>
        </div>
        <p>
          Matches {stored.match === "all" ? "all" : "any"} of: {" "}
          <strong>{stored.include_tags.join(", ") || "all tags"}</strong>
          {stored.exclude_tags.length > 0
            ? ` · excludes ${stored.exclude_tags.join(", ")}`
            : ""}
        </p>
        <p className="muted small">
          The local rule refreshes before this playlist is opened or played. Model
          suggestions are never used as automatic evidence.
        </p>
        <div className="form-actions">
          <button type="button" onClick={() => setEditing(true)}>
            Edit rule
          </button>
          <button type="button" disabled={refreshing} onClick={() => void refreshNow()}>
            {refreshing ? "Refreshing…" : "Refresh now"}
          </button>
          <button type="button" onClick={() => void makeManual()}>
            Make manual
          </button>
        </div>
      </section>
    );
  }

  if (playlist.automatic && !editing && stored === null) {
    return (
      <section className="playlist-automatic-editor authoring-card" role="alert">
        <div className="playlist-automatic-heading">
          <div>
            <p className="assistant-eyebrow">Automatic playlist needs attention</p>
            <h3>The saved rule cannot be read</h3>
          </div>
          <span>{playlist.automatic_rule_error ?? "automatic_rule_invalid"}</span>
        </div>
        <p className="muted small">
          The last resolved songs are being kept so playback still works. Replace the
          rule through a new preview, or make the playlist manual to keep those songs.
        </p>
        <div className="form-actions">
          <button type="button" onClick={() => setEditing(true)}>
            Replace rule
          </button>
          <button type="button" onClick={() => void makeManual()}>
            Make manual
          </button>
        </div>
      </section>
    );
  }

  return (
    <section className="playlist-automatic-editor authoring-card">
      <div className="playlist-automatic-heading">
        <div>
          <p className="assistant-eyebrow">
            {playlist.automatic ? "Edit automatic rule" : "Optional automation"}
          </p>
          <h3>Fill this normal playlist from local tags</h3>
        </div>
        {playlist.automatic ? (
          <button type="button" onClick={() => setEditing(false)}>Cancel editing</button>
        ) : null}
      </div>
      <p className="muted small">
        Preview is read-only. The playlist changes only after you review the resolved songs
        and explicitly save the rule.
      </p>
      <div className="playlist-automatic-fields">
        <Field label="Include tags (comma separated)">
          <input
            value={includeTags}
            placeholder="medieval, tavern, dancing"
            onChange={(event) => setIncludeTags(event.target.value)}
          />
        </Field>
        <Field label="Tag matching">
          <select value={match} onChange={(event) => setMatch(event.target.value as "any" | "all")}>
            <option value="any">Match any included tag</option>
            <option value="all">Match every included tag</option>
          </select>
        </Field>
        <Field label="Exclude tags (comma separated)">
          <input
            value={excludeTags}
            placeholder="combat, tense"
            onChange={(event) => setExcludeTags(event.target.value)}
          />
        </Field>
        <Field label="Tag evidence">
          <select
            value={tagSources}
            onChange={(event) =>
              setTagSources(event.target.value as "manual" | "manual_and_local")
            }
          >
            <option value="manual">Database mood tags only</option>
            <option value="manual_and_local">Manual + current local analysis</option>
          </select>
        </Field>
        <Field label="Minimum BPM">
          <input type="number" min={1} max={999} value={minBpm} onChange={(event) => setMinBpm(event.target.value)} />
        </Field>
        <Field label="Maximum BPM">
          <input type="number" min={1} max={999} value={maxBpm} onChange={(event) => setMaxBpm(event.target.value)} />
        </Field>
        <Field label="Maximum songs">
          <input type="number" min={1} max={1000} value={maximumTracks} onChange={(event) => setMaximumTracks(Number(event.target.value))} />
        </Field>
        <Field label="Order">
          <select value={orderBy} onChange={(event) => setOrderBy(event.target.value as AutomaticPlaylistRule["order_by"])}>
            <option value="title">Title</option>
            <option value="newest">Newest added</option>
            <option value="bpm_ascending">BPM, low to high</option>
            <option value="bpm_descending">BPM, high to low</option>
          </select>
        </Field>
      </div>
      <label className="assistant-check-row">
        <input type="checkbox" checked={includeUnknownBpm} onChange={(event) => setIncludeUnknownBpm(event.target.checked)} />
        Include songs without a BPM value
      </label>
      <div className="form-actions">
        <button type="button" className="btn-primary" disabled={previewing} onClick={() => void previewRule()}>
          {previewing ? "Resolving…" : "Preview matching songs"}
        </button>
      </div>

      {preview !== null ? (
        <div className="playlist-automatic-preview">
          <div>
            <strong>{preview.matched_tracks} matching songs</strong>
            <span> from {preview.library_tracks} library tracks</span>
          </div>
          {preview.tracks.length === 0 ? (
            <p className="muted small">No songs currently match this rule.</p>
          ) : (
            <ol>
              {preview.tracks.slice(0, 50).map((track) => (
                <li key={track.id}>
                  <span>{track.title || track.path}</span>
                  <small>{track.artist || "Unknown artist"}{track.bpm ? ` · ${track.bpm} BPM` : ""}</small>
                </li>
              ))}
            </ol>
          )}
          {preview.tracks.length > 50 ? (
            <p className="muted small">And {preview.tracks.length - 50} more songs.</p>
          ) : null}
          <button type="button" className="btn-primary" disabled={saving} onClick={() => void saveRule()}>
            {saving
              ? "Saving…"
              : playlist.automatic
                ? "Save updated automatic rule"
                : "Enable automatic playlist"}
          </button>
        </div>
      ) : null}
    </section>
  );
}
