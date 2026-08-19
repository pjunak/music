import { type FormEvent, useEffect, useMemo, useState } from "react";

import { confirmDialog } from "@/components/confirmDialog";
import { EmptyState } from "@/components/EmptyState";
import {
  type AnalysisTagReviewDecision,
  type AnalysisTagReviewResult,
  type AnalysisTagReviewTarget,
  type AnalysisTagSuggestion,
  type BulkAnalysisTagReviewDecision,
  type LibraryTagPage,
  type LibraryTagTrack,
  type ManualTagCatalog,
  assistantApi,
} from "@/core/api";
import { toast } from "@/core/toast";

import { AnalysisTagReview } from "./AnalysisTagReview";
import { analysisTagSuggestionKey } from "./analysisTagSelection";
import { AudioSignalEvidence } from "./AudioSignalEvidence";
import { TagCatalogManager } from "./TagCatalogManager";

const PAGE_SIZE = 50;
const MAX_TAGS = 32;
const MAX_TAG_LENGTH = 64;
const MAX_BULK_REVIEW_ITEMS = 1000;

function displayName(track: LibraryTagTrack): string {
  return track.display_title || track.title || track.path;
}

function pendingSuggestionCount(track: LibraryTagTrack): number {
  return track.analysis_suggestions.filter(
    (suggestion) => suggestion.status === "pending",
  ).length;
}

function normalizeTag(value: string): string {
  return value.normalize("NFKC").trim().replace(/\s+/g, " ").toLowerCase();
}

function sortedUnique(tags: readonly string[]): string[] {
  return [...new Set(tags)].sort((left, right) => left.localeCompare(right));
}

function sameTags(left: readonly string[], right: readonly string[]): boolean {
  return left.length === right.length && left.every((tag, index) => tag === right[index]);
}

interface LibraryTagEditorProps {
  refreshKey?: number;
}

export function LibraryTagEditor({ refreshKey = 0 }: LibraryTagEditorProps) {
  const [catalog, setCatalog] = useState<ManualTagCatalog | null>(null);
  const [page, setPage] = useState<LibraryTagPage>({
    items: [],
    total: 0,
    offset: 0,
    limit: PAGE_SIZE,
  });
  const [searchDraft, setSearchDraft] = useState("");
  const [search, setSearch] = useState("");
  const [tagFilter, setTagFilter] = useState("");
  const [reviewFilter, setReviewFilter] = useState<
    "" | AnalysisTagReviewDecision
  >("");
  const [offset, setOffset] = useState(0);
  const [selectedId, setSelectedId] = useState<number | null>(null);
  const [selectedTrackIds, setSelectedTrackIds] = useState<Set<number>>(new Set());
  const [selectedReviewItems, setSelectedReviewItems] = useState<
    Map<string, AnalysisTagReviewTarget>
  >(new Map());
  const [draftTags, setDraftTags] = useState<string[]>([]);
  const [customTag, setCustomTag] = useState("");
  const [bulkTags, setBulkTags] = useState<string[]>([]);
  const [bulkCustomTag, setBulkCustomTag] = useState("");
  const [loading, setLoading] = useState(true);
  const [saving, setSaving] = useState(false);
  const [bulkSaving, setBulkSaving] = useState(false);
  const [bulkReviewSaving, setBulkReviewSaving] = useState(false);
  const [catalogError, setCatalogError] = useState<string | null>(null);
  const [listError, setListError] = useState<string | null>(null);
  const [reloadKey, setReloadKey] = useState(0);

  useEffect(() => {
    let disposed = false;
    void assistantApi
      .getManualTagCatalog()
      .then((response) => {
        if (!disposed) {
          setCatalog(response);
          setCatalogError(null);
        }
      })
      .catch((error: unknown) => {
        if (!disposed) {
          setCatalogError(
            error instanceof Error ? error.message : "Tag catalog is unavailable.",
          );
        }
      });
    return () => {
      disposed = true;
    };
  }, [refreshKey, reloadKey]);

  useEffect(() => {
    let disposed = false;
    setLoading(true);
    void assistantApi
      .listLibraryTags({
        ...(search ? { search } : {}),
        ...(tagFilter ? { tag: tagFilter } : {}),
        ...(reviewFilter ? { review: reviewFilter } : {}),
        offset,
        limit: PAGE_SIZE,
      })
      .then((response) => {
        if (disposed) return;
        setPage(response);
        setListError(null);
        setSelectedId((current) =>
          response.items.some((track) => track.track_id === current)
            ? current
            : (response.items[0]?.track_id ?? null),
        );
      })
      .catch((error: unknown) => {
        if (!disposed) {
          setListError(
            error instanceof Error ? error.message : "Library tags are unavailable.",
          );
        }
      })
      .finally(() => {
        if (!disposed) setLoading(false);
      });
    return () => {
      disposed = true;
    };
  }, [offset, refreshKey, reloadKey, reviewFilter, search, tagFilter]);

  const selected = useMemo(
    () => page.items.find((track) => track.track_id === selectedId),
    [page.items, selectedId],
  );
  useEffect(() => {
    setDraftTags(sortedUnique(selected?.manual_tags ?? []));
    setCustomTag("");
  }, [selected]);

  const filterTags = useMemo(
    () => sortedUnique(catalog?.used_tags ?? []),
    [catalog],
  );
  const originalTags = useMemo(
    () => sortedUnique(selected?.manual_tags ?? []),
    [selected],
  );
  const dirty = selected !== undefined && !sameTags(draftTags, originalTags);
  const loadError = listError ?? catalogError;
  const selectedReviewKeys = useMemo(
    () => new Set(selectedReviewItems.keys()),
    [selectedReviewItems],
  );

  function submitSearch(event: FormEvent) {
    event.preventDefault();
    setOffset(0);
    setSearch(searchDraft.trim());
    setSelectedTrackIds(new Set());
    setSelectedReviewItems(new Map());
  }

  function toggleTag(tag: string) {
    setDraftTags((current) =>
      current.includes(tag)
        ? current.filter((item) => item !== tag)
        : sortedUnique([...current, tag]),
    );
  }

  function addCustomTags(event: FormEvent) {
    event.preventDefault();
    const additions = customTag
      .split(",")
      .map(normalizeTag)
      .filter(Boolean);
    if (additions.length === 0) return;
    if (additions.some((tag) => tag.length > MAX_TAG_LENGTH)) {
      toast.error(
        "Tag is too long",
        `Each manual tag can contain at most ${MAX_TAG_LENGTH} characters.`,
      );
      return;
    }
    const next = sortedUnique([...draftTags, ...additions]);
    if (next.length > MAX_TAGS) {
      toast.error("Too many tags", `A track can have at most ${MAX_TAGS} manual tags.`);
      return;
    }
    setDraftTags(next);
    setCustomTag("");
  }

  function addBulkCustomTags(event: FormEvent) {
    event.preventDefault();
    const additions = bulkCustomTag
      .split(",")
      .map(normalizeTag)
      .filter(Boolean);
    if (additions.length === 0) return;
    if (additions.some((tag) => tag.length > MAX_TAG_LENGTH)) {
      toast.error(
        "Tag is too long",
        `Each manual tag can contain at most ${MAX_TAG_LENGTH} characters.`,
      );
      return;
    }
    const next = sortedUnique([...bulkTags, ...additions]);
    if (next.length > MAX_TAGS) {
      toast.error("Too many tags", `Choose at most ${MAX_TAGS} tags at once.`);
      return;
    }
    setBulkTags(next);
    setBulkCustomTag("");
  }

  function toggleTrackSelection(trackId: number) {
    setSelectedTrackIds((current) => {
      const next = new Set(current);
      if (next.has(trackId)) next.delete(trackId);
      else next.add(trackId);
      return next;
    });
  }

  function selectCurrentPage() {
    setSelectedTrackIds((current) => {
      const next = new Set(current);
      for (const track of page.items) next.add(track.track_id);
      return next;
    });
  }

  function selectAnalysisSuggestion(
    trackId: number,
    suggestion: AnalysisTagSuggestion,
    isSelected: boolean,
  ) {
    const key = analysisTagSuggestionKey(trackId, suggestion);
    setSelectedReviewItems((current) => {
      if (
        isSelected &&
        !current.has(key) &&
        current.size >= MAX_BULK_REVIEW_ITEMS
      ) {
        toast.error(
          "Bulk review selection is full",
          `Review at most ${MAX_BULK_REVIEW_ITEMS} suggestions in one batch.`,
        );
        return current;
      }
      const next = new Map(current);
      if (isSelected) {
        next.set(key, {
          track_id: trackId,
          tag: suggestion.tag,
          analyzer_id: suggestion.analyzer_id,
          source_signature: suggestion.source_signature,
        });
      } else {
        next.delete(key);
      }
      return next;
    });
  }

  async function applyBulkReview(decision: BulkAnalysisTagReviewDecision) {
    if (selectedReviewItems.size === 0 || dirty) return;
    const count = selectedReviewItems.size;
    const confirmed = await confirmDialog({
      title:
        decision === "accepted"
          ? "Add selected suggestions to your tags?"
          : "Reject selected suggestions?",
      body:
        decision === "accepted"
          ? `${count} selected ${count === 1 ? "suggestion" : "suggestions"} will be copied into your manual tags.`
          : `${count} selected ${count === 1 ? "suggestion" : "suggestions"} will stop contributing tag labels to playlist matches. You can reopen decisions later.`,
      confirmLabel: decision === "accepted" ? "Add selected tags" : "Reject selected",
    });
    if (!confirmed) return;

    setBulkReviewSaving(true);
    try {
      const result = await assistantApi.reviewAnalysisTagsBulk(
        [...selectedReviewItems.values()],
        decision,
      );
      if (result.failures.length > 0) {
        const details = result.failures
          .slice(0, 3)
          .map((failure) => `#${failure.track_id} “${failure.tag}”: ${failure.error}`)
          .join("; ");
        const remainder =
          result.failures.length > 3
            ? `; +${result.failures.length - 3} more`
            : "";
        toast.error(
          "Bulk review partly applied",
          `${result.applied.length} applied; ${result.failures.length} skipped. ${details}${remainder}`,
        );
      } else {
        toast.success(
          decision === "accepted" ? "Suggestions accepted" : "Suggestions rejected",
          `${result.applied.length} ${result.applied.length === 1 ? "decision was" : "decisions were"} saved.`,
        );
      }
      setSelectedReviewItems(new Map());
      setReloadKey((value) => value + 1);
    } catch (error) {
      toast.error(
        "Bulk review could not be saved",
        error instanceof Error ? error.message : undefined,
      );
    } finally {
      setBulkReviewSaving(false);
    }
  }

  async function applyBulk(mode: "add" | "remove") {
    if (selectedTrackIds.size === 0 || bulkTags.length === 0) return;
    setBulkSaving(true);
    try {
      const result = await assistantApi.patchManualTagsBulk(
        [...selectedTrackIds],
        mode === "add" ? bulkTags : [],
        mode === "remove" ? bulkTags : [],
      );
      const skipped = result.missing_track_ids.length + result.failures.length;
      if (skipped > 0) {
        const skippedIds = [
          ...result.missing_track_ids,
          ...result.failures.map((failure) => failure.track_id),
        ];
        const visibleIds = skippedIds.slice(0, 5).join(", ");
        const remainder = skippedIds.length > 5 ? `, +${skippedIds.length - 5} more` : "";
        toast.error(
          "Bulk tagging partly applied",
          `${result.changed_track_ids.length} tracks changed; ${skipped} skipped (IDs ${visibleIds}${remainder}).`,
        );
      } else {
        toast.success(
          mode === "add" ? "Tags added" : "Tags removed",
          result.changed_track_ids.length === 0
            ? "The selected tracks were already up to date."
            : `${result.changed_track_ids.length} ${result.changed_track_ids.length === 1 ? "track was" : "tracks were"} updated.`,
        );
      }
      setBulkTags([]);
      setReloadKey((value) => value + 1);
    } catch (error) {
      toast.error(
        "Bulk tags could not be applied",
        error instanceof Error ? error.message : undefined,
      );
    } finally {
      setBulkSaving(false);
    }
  }

  async function save() {
    if (selected === undefined || !dirty) return;
    const add = draftTags.filter((tag) => !originalTags.includes(tag));
    const remove = originalTags.filter((tag) => !draftTags.includes(tag));
    setSaving(true);
    try {
      const updated = await assistantApi.patchManualTags(selected.track_id, add, remove);
      setPage((current) => ({
        ...current,
        items: current.items.map((track) =>
          track.track_id === updated.track_id ? updated : track,
        ),
      }));
      setDraftTags(sortedUnique(updated.manual_tags));
      setCatalog((current) =>
        current === null
          ? current
          : {
              ...current,
              used_tags: sortedUnique([...current.used_tags, ...updated.manual_tags]),
            },
      );
      void assistantApi
        .getManualTagCatalog()
        .then((refreshed) => {
          setCatalog(refreshed);
          setCatalogError(null);
        })
        .catch((error: unknown) => {
          setCatalogError(
            error instanceof Error ? error.message : "Tag catalog is unavailable.",
          );
        });
      if (tagFilter && !updated.manual_tags.includes(tagFilter)) {
        setReloadKey((value) => value + 1);
      }
      toast.success("Manual tags saved", "Playlist suggestions use them immediately.");
    } catch (error) {
      toast.error(
        "Tags could not be saved",
        error instanceof Error ? error.message : undefined,
      );
    } finally {
      setSaving(false);
    }
  }

  function handleAnalysisReviewed(result: AnalysisTagReviewResult) {
    setSelectedReviewItems((current) => {
      const next = new Map(current);
      next.delete(analysisTagSuggestionKey(result.track_id, result));
      return next;
    });
    setPage((current) => ({
      ...current,
      items: current.items.map((track) =>
        track.track_id === result.track_id
          ? {
              ...track,
              manual_tags: sortedUnique(result.manual_tags),
              analysis_suggestions: track.analysis_suggestions.map((suggestion) =>
                suggestion.analyzer_id === result.analyzer_id &&
                suggestion.source_signature === result.source_signature &&
                suggestion.tag === result.tag
                  ? { ...suggestion, status: result.decision }
                  : suggestion,
              ),
            }
          : track,
      ),
    }));
    if (result.decision === "accepted") {
      void assistantApi
        .getManualTagCatalog()
        .then((refreshed) => {
          setCatalog(refreshed);
          setCatalogError(null);
        })
        .catch((error: unknown) => {
          setCatalogError(
            error instanceof Error ? error.message : "Tag catalog is unavailable.",
          );
        });
    }
    if (reviewFilter) setReloadKey((value) => value + 1);
  }

  return (
    <section className="surface-card assistant-tag-workspace">
      <div className="assistant-section-heading">
        <div>
          <p className="assistant-eyebrow">Human-owned library context</p>
          <h2>Manual playlist tags</h2>
          <p>
            Add your own D&amp;D settings, scenes, and moods. These labels stay
            separate from local and model-generated suggestions.
          </p>
        </div>
        <span>{page.total} tracks</span>
      </div>

      <div className="assistant-tag-toolbar">
        <form onSubmit={submitSearch} role="search">
          <input
            value={searchDraft}
            onChange={(event) => setSearchDraft(event.target.value)}
            aria-label="Search tracks to tag"
            placeholder="Search title, artist, album, or path"
          />
          <button type="submit">Search</button>
        </form>
        <label>
          <span>Filter by your tag</span>
          <select
            value={tagFilter}
            onChange={(event) => {
              setTagFilter(event.target.value);
              setOffset(0);
              setSelectedTrackIds(new Set());
              setSelectedReviewItems(new Map());
            }}
          >
            <option value="">All manual tags</option>
            {filterTags.map((tag) => (
              <option key={tag} value={tag}>
                {tag}
              </option>
            ))}
          </select>
        </label>
        <label>
          <span>Filter analysis review</span>
          <select
            value={reviewFilter}
            onChange={(event) => {
              setReviewFilter(event.target.value as "" | AnalysisTagReviewDecision);
              setOffset(0);
              setSelectedTrackIds(new Set());
              setSelectedReviewItems(new Map());
            }}
          >
            <option value="">All review states</option>
            <option value="pending">Needs review</option>
            <option value="accepted">Accepted suggestions</option>
            <option value="rejected">Rejected suggestions</option>
          </select>
        </label>
      </div>

      {loadError !== null ? (
        <div className="assistant-analysis-error" role="alert">
          <span>{loadError}</span>
          <button type="button" onClick={() => setReloadKey((value) => value + 1)}>
            Retry
          </button>
        </div>
      ) : null}

      {selectedReviewItems.size > 0 ? (
        <div
          className="assistant-bulk-reviews"
          role="region"
          aria-label="Bulk analysis review"
        >
          <div>
            <strong>{selectedReviewItems.size} suggestions selected</strong>
            <span>
              Apply one explicit decision to this selection. Invalid or stale items
              will be reported without blocking valid ones.
            </span>
          </div>
          {dirty ? (
            <p className="assistant-review-note">
              Save or discard the open manual-tag edits before applying this batch.
            </p>
          ) : null}
          <div className="assistant-bulk-actions">
            <button
              type="button"
              className="btn-ghost"
              disabled={bulkReviewSaving}
              onClick={() => setSelectedReviewItems(new Map())}
            >
              Clear selection
            </button>
            <button
              type="button"
              disabled={dirty || bulkReviewSaving}
              onClick={() => void applyBulkReview("rejected")}
            >
              Reject selected
            </button>
            <button
              type="button"
              className="btn-primary"
              disabled={dirty || bulkReviewSaving}
              onClick={() => void applyBulkReview("accepted")}
            >
              {bulkReviewSaving ? "Applying…" : "Add selected to my tags"}
            </button>
          </div>
        </div>
      ) : null}

      {selectedTrackIds.size > 0 ? (
        <div className="assistant-bulk-tags" role="region" aria-label="Bulk tag editor">
          <div className="assistant-bulk-tags-heading">
            <div>
              <strong>{selectedTrackIds.size} tracks selected</strong>
              <span>Choose tags, then add or remove them across the selection.</span>
            </div>
            <button
              type="button"
              className="btn-ghost"
              onClick={() => setSelectedTrackIds(new Set())}
            >
              Clear selection
            </button>
          </div>
          <div className="assistant-editable-tags">
            {bulkTags.length === 0 ? (
              <span className="muted small">No bulk tags chosen yet.</span>
            ) : (
              bulkTags.map((tag) => (
                <button
                  type="button"
                  onClick={() =>
                    setBulkTags((current) => current.filter((item) => item !== tag))
                  }
                  aria-label={`Remove bulk tag ${tag}`}
                  key={tag}
                >
                  {tag} <span aria-hidden="true">×</span>
                </button>
              ))
            )}
          </div>
          <form className="assistant-custom-tag" onSubmit={addBulkCustomTags}>
            <input
              value={bulkCustomTag}
              onChange={(event) => setBulkCustomTag(event.target.value)}
              aria-label="Choose custom bulk tags"
              placeholder="Tags for this batch, separated by commas"
              maxLength={256}
            />
            <button type="submit">Choose tags</button>
          </form>
          <div className="assistant-bulk-starters">
            {catalog?.starter_groups.flatMap((group) => group.tags).map((tag) => (
              <button
                type="button"
                className="btn-toggle"
                aria-pressed={bulkTags.includes(tag)}
                onClick={() =>
                  setBulkTags((current) =>
                    current.includes(tag)
                      ? current.filter((item) => item !== tag)
                      : sortedUnique([...current, tag]),
                  )
                }
                key={tag}
              >
                {tag}
              </button>
            ))}
          </div>
          <div className="assistant-bulk-actions">
            <button
              type="button"
              disabled={bulkTags.length === 0 || bulkSaving}
              onClick={() => void applyBulk("remove")}
            >
              Remove from selected
            </button>
            <button
              type="button"
              className="btn-primary"
              disabled={bulkTags.length === 0 || bulkSaving}
              onClick={() => void applyBulk("add")}
            >
              {bulkSaving ? "Applying…" : "Add to selected"}
            </button>
          </div>
        </div>
      ) : null}

      <div className="assistant-tag-layout">
        <div className="assistant-tag-track-panel">
          {loading ? (
            <p className="muted">Loading tracks…</p>
          ) : page.items.length === 0 ? (
            <EmptyState title="No matching tracks">
              Clear the search or tag filter to see more of the library.
            </EmptyState>
          ) : (
            <div className="assistant-tag-track-list" role="list" aria-label="Tracks">
              {page.items.map((track) => (
                <div
                  className={`assistant-tag-track-row${track.track_id === selectedId ? " is-focused" : ""}`}
                  key={track.track_id}
                >
                  <input
                    type="checkbox"
                    checked={selectedTrackIds.has(track.track_id)}
                    onChange={() => toggleTrackSelection(track.track_id)}
                    aria-label={`Select ${displayName(track)} for bulk tagging`}
                  />
                  <button
                    type="button"
                    className="assistant-tag-track-main"
                    onClick={() => setSelectedId(track.track_id)}
                  >
                    <strong>{displayName(track)}</strong>
                    <span>{track.artist || track.album || track.path}</span>
                    <span className="assistant-track-tag-preview">
                      {track.manual_tags.length > 0
                        ? track.manual_tags.join(" · ")
                        : "No manual tags"}
                    </span>
                    {pendingSuggestionCount(track) > 0 ? (
                      <span className="assistant-track-review-count">
                        {pendingSuggestionCount(track)} to review
                      </span>
                    ) : null}
                  </button>
                </div>
              ))}
            </div>
          )}
          <div className="assistant-tag-pagination">
            <button
              type="button"
              disabled={offset === 0 || loading}
              onClick={() => setOffset(Math.max(0, offset - PAGE_SIZE))}
            >
              Previous
            </button>
            <span>
              {page.total === 0 ? 0 : page.offset + 1}–
              {Math.min(page.offset + page.items.length, page.total)} of {page.total}
            </span>
            <button
              type="button"
              disabled={offset + page.items.length >= page.total || loading}
              onClick={() => setOffset(offset + PAGE_SIZE)}
            >
              Next
            </button>
          </div>
          {page.items.length > 0 ? (
            <button
              type="button"
              className="btn-ghost assistant-select-page"
              onClick={selectCurrentPage}
            >
              Select this page
            </button>
          ) : null}
        </div>

        <div className="assistant-tag-editor">
          {selected === undefined ? (
            <EmptyState title="Select a track to tag">
              Manual and generated labels will be shown in separate sections.
            </EmptyState>
          ) : (
            <>
              <div className="assistant-tag-editor-heading">
                <div>
                  <h3>{displayName(selected)}</h3>
                  <span>{selected.path}</span>
                </div>
                {dirty ? <span className="assistant-unsaved">Unsaved changes</span> : null}
              </div>

              <div className="assistant-tag-source is-manual">
                <div>
                  <strong>Your tags</strong>
                  <span>Editable and always preferred as explicit human context.</span>
                </div>
                <div className="assistant-editable-tags">
                  {draftTags.length === 0 ? (
                    <span className="muted small">No manual tags yet.</span>
                  ) : (
                    draftTags.map((tag) => (
                      <button
                        type="button"
                        onClick={() => toggleTag(tag)}
                        aria-label={`Remove tag ${tag}`}
                        key={tag}
                      >
                        {tag} <span aria-hidden="true">×</span>
                      </button>
                    ))
                  )}
                </div>
                <form className="assistant-custom-tag" onSubmit={addCustomTags}>
                  <input
                    value={customTag}
                    onChange={(event) => setCustomTag(event.target.value)}
                    aria-label="Create custom tags"
                    placeholder="Custom tag, or several separated by commas"
                    maxLength={256}
                  />
                  <button type="submit">Add</button>
                </form>
              </div>

              <div className="assistant-starter-tags">
                {catalog?.starter_groups.map((group) => (
                  <div key={group.key}>
                    <strong>{group.label}</strong>
                    <div>
                      {group.tags.map((tag) => {
                        const selectedTag = draftTags.includes(tag);
                        return (
                          <button
                            type="button"
                            className="btn-toggle"
                            aria-pressed={selectedTag}
                            onClick={() => toggleTag(tag)}
                            key={tag}
                          >
                            {tag}
                          </button>
                        );
                      })}
                    </div>
                  </div>
                ))}
              </div>

              <AnalysisTagReview
                trackId={selected.track_id}
                suggestions={selected.analysis_suggestions}
                selectedSuggestionKeys={selectedReviewKeys}
                disabled={dirty || saving}
                onReviewed={handleAnalysisReviewed}
                onSelectionChange={(suggestion, isSelected) =>
                  selectAnalysisSuggestion(
                    selected.track_id,
                    suggestion,
                    isSelected,
                  )
                }
              />

              <AudioSignalEvidence profile={selected.audio_signal} />

              <div className="assistant-tag-save-row">
                <button
                  type="button"
                  className="btn-ghost"
                  disabled={!dirty || saving}
                  onClick={() => setDraftTags(originalTags)}
                >
                  Discard
                </button>
                <button
                  type="button"
                  className="btn-primary"
                  disabled={!dirty || saving}
                  onClick={() => void save()}
                >
                  {saving ? "Saving…" : "Save manual tags"}
                </button>
              </div>
            </>
          )}
        </div>
      </div>

      <TagCatalogManager
        catalog={catalog}
        onChanged={() => {
          setTagFilter("");
          setOffset(0);
          setSelectedTrackIds(new Set());
          setReloadKey((value) => value + 1);
        }}
      />
    </section>
  );
}
