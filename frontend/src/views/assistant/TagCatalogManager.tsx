import { useEffect, useState } from "react";

import { confirmDialog } from "@/components/confirmDialog";
import { inputDialog } from "@/components/inputDialog";
import type { ManualTagCatalog, TagCleanupPreview } from "@/core/api";
import { assistantApi } from "@/core/api";
import { toast } from "@/core/toast";

const MAX_TAG_LENGTH = 64;

interface Props {
  catalog: ManualTagCatalog | null;
  onChanged: () => void;
}

function normalizeTag(value: string): string {
  return value.normalize("NFKC").trim().replace(/\s+/g, " ").toLowerCase();
}

export function TagCatalogManager({ catalog, onChanged }: Props) {
  const [busyTag, setBusyTag] = useState<string | null>(null);
  const [cleanupPreview, setCleanupPreview] = useState<TagCleanupPreview | null>(
    null,
  );
  const [selectedCleanupIds, setSelectedCleanupIds] = useState<Set<string>>(
    new Set(),
  );
  const [cleanupLoading, setCleanupLoading] = useState(false);
  const [cleanupApplying, setCleanupApplying] = useState(false);
  const [cleanupError, setCleanupError] = useState<string | null>(null);

  useEffect(() => {
    setCleanupPreview(null);
    setSelectedCleanupIds(new Set());
    setCleanupError(null);
  }, [catalog]);

  async function findCleanupSuggestions() {
    setCleanupLoading(true);
    setCleanupError(null);
    try {
      const preview = await assistantApi.previewTagCleanup();
      setCleanupPreview(preview);
      setSelectedCleanupIds(new Set());
    } catch (error) {
      setCleanupError(
        error instanceof Error ? error.message : "Cleanup suggestions are unavailable.",
      );
    } finally {
      setCleanupLoading(false);
    }
  }

  function toggleCleanupSuggestion(id: string) {
    setSelectedCleanupIds((current) => {
      const next = new Set(current);
      if (next.has(id)) next.delete(id);
      else next.add(id);
      return next;
    });
  }

  async function applyCleanupSuggestions() {
    if (cleanupPreview === null || selectedCleanupIds.size === 0) return;
    const selected = cleanupPreview.suggestions.filter((item) =>
      selectedCleanupIds.has(item.id),
    );
    const confirmed = await confirmDialog({
      title: "Apply selected tag cleanup?",
      body: `${selected.length} selected ${selected.length === 1 ? "rename" : "renames"} will be applied across the library in one update. Unselected suggestions will remain unchanged.`,
      confirmLabel: "Apply selected",
      tone: "primary",
    });
    if (!confirmed) return;

    setCleanupApplying(true);
    try {
      const result = await assistantApi.applyTagCleanup(
        cleanupPreview.catalog_signature,
        cleanupPreview.vocabulary_fingerprint,
        selected.map((item) => ({ source: item.source, target: item.target })),
      );
      const affectedTracks = result.applied.reduce(
        (total, item) => total + item.affected_tracks,
        0,
      );
      toast.success(
        "Selected tag cleanup applied",
        `${result.applied.length} ${result.applied.length === 1 ? "tag was" : "tags were"} cleaned, updating ${affectedTracks} ${affectedTracks === 1 ? "tag assignment" : "tag assignments"}.`,
      );
      setCleanupPreview(null);
      setSelectedCleanupIds(new Set());
      onChanged();
    } catch (error) {
      toast.error(
        "Tag cleanup could not be applied",
        error instanceof Error ? error.message : undefined,
      );
    } finally {
      setCleanupApplying(false);
    }
  }

  async function rename(source: string, trackCount: number) {
    const entered = await inputDialog({
      title: "Rename or merge tag",
      body: `“${source}” is used by ${trackCount} ${trackCount === 1 ? "track" : "tracks"}.`,
      label: "New tag name",
      initial: source,
      confirmLabel: "Continue",
      validate: (value) => {
        const normalized = normalizeTag(value);
        if (!normalized) return "Enter a tag name.";
        if (normalized.length > MAX_TAG_LENGTH) {
          return `Use at most ${MAX_TAG_LENGTH} characters.`;
        }
        if (normalized === source) return "Enter a different tag name.";
        return null;
      },
    });
    if (entered === null) return;
    const target = normalizeTag(entered);
    const targetUsage = catalog?.tag_usage.find((item) => item.tag === target);
    const merged = targetUsage !== undefined;
    const confirmed = await confirmDialog({
      title: merged ? "Merge these tags?" : "Rename this tag?",
      body: merged
        ? `Every “${source}” tag will become “${target}”. Tracks that already have both will keep one “${target}” tag.`
        : `Replace “${source}” with “${target}” on ${trackCount} ${trackCount === 1 ? "track" : "tracks"}.`,
      confirmLabel: merged ? "Merge tags" : "Rename tag",
      tone: "primary",
    });
    if (!confirmed) return;

    setBusyTag(source);
    try {
      const result = await assistantApi.renameManualTag(source, target);
      toast.success(
        result.merged ? "Tags merged" : "Tag renamed",
        `${result.affected_tracks} ${result.affected_tracks === 1 ? "track was" : "tracks were"} updated.`,
      );
      onChanged();
    } catch (error) {
      toast.error(
        "Tag could not be changed",
        error instanceof Error ? error.message : undefined,
      );
    } finally {
      setBusyTag(null);
    }
  }

  return (
    <details className="assistant-tag-catalog-manager">
      <summary>
        <span>Manage used tags</span>
        <span>{catalog?.tag_usage.length ?? 0} tags</span>
      </summary>
      <p>
        Rename a tag across the library. Choosing an existing name merges the two
        tags without creating duplicates.
      </p>
      <div className="assistant-tag-cleanup">
        <div className="assistant-tag-cleanup-heading">
          <div>
            <strong>Cleanup suggestions</strong>
            <span>
              Find declared aliases plus clear spelling or plural matches to the
              controlled vocabulary. Nothing changes until you select and confirm it.
            </span>
          </div>
          <button
            type="button"
            className="btn-secondary"
            disabled={
              catalog === null || cleanupLoading || cleanupApplying || busyTag !== null
            }
            onClick={() => void findCleanupSuggestions()}
          >
            {cleanupLoading
              ? "Checking…"
              : cleanupPreview === null
                ? "Find suggestions"
                : "Check again"}
          </button>
        </div>
        {cleanupError !== null ? (
          <p className="assistant-provider-problem" role="alert">
            {cleanupError}
          </p>
        ) : null}
        {cleanupPreview !== null ? (
          cleanupPreview.suggestions.length === 0 ? (
            <p className="muted">No clear cleanup suggestions were found.</p>
          ) : (
            <>
              <div
                className="assistant-tag-cleanup-list"
                role="group"
                aria-label="Tag cleanup suggestions"
              >
                {cleanupPreview.suggestions.map((item) => (
                  <label key={item.id}>
                    <input
                      type="checkbox"
                      checked={selectedCleanupIds.has(item.id)}
                      disabled={cleanupApplying}
                      onChange={() => toggleCleanupSuggestion(item.id)}
                    />
                    <span>
                      <strong>
                        {item.source} <span aria-hidden="true">→</span> {item.target}
                      </strong>
                      <small>
                        {item.reason} {item.source_track_count}{" "}
                        {item.source_track_count === 1 ? "track" : "tracks"}
                        {item.merged ? " · merges with an existing tag" : ""}
                      </small>
                    </span>
                  </label>
                ))}
              </div>
              <div className="assistant-tag-cleanup-actions">
                <span>{selectedCleanupIds.size} selected</span>
                <button
                  type="button"
                  className="btn-primary"
                  disabled={selectedCleanupIds.size === 0 || cleanupApplying}
                  onClick={() => void applyCleanupSuggestions()}
                >
                  {cleanupApplying ? "Applying…" : "Apply selected"}
                </button>
              </div>
            </>
          )
        ) : null}
      </div>
      {catalog === null ? (
        <p className="muted">Loading tag usage…</p>
      ) : catalog.tag_usage.length === 0 ? (
        <p className="muted">No manual tags have been used yet.</p>
      ) : (
        <div className="assistant-tag-usage-list">
          {catalog.tag_usage.map((item) => (
            <div key={item.tag}>
              <strong>{item.tag}</strong>
              <span>
                {item.track_count} {item.track_count === 1 ? "track" : "tracks"}
              </span>
              <button
                type="button"
                className="btn-ghost"
                disabled={busyTag !== null || cleanupApplying}
                onClick={() => void rename(item.tag, item.track_count)}
                aria-label={`Rename or merge ${item.tag}`}
              >
                {busyTag === item.tag ? "Updating…" : "Rename / merge"}
              </button>
            </div>
          ))}
        </div>
      )}
    </details>
  );
}
