import { useState } from "react";

import { confirmDialog } from "@/components/confirmDialog";
import { inputDialog } from "@/components/inputDialog";
import type { ManualTagCatalog } from "@/core/api";
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
                disabled={busyTag !== null}
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
