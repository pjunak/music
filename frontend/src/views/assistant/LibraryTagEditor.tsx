import { type FormEvent, useCallback, useEffect, useMemo, useState } from "react";

import { confirmDialog } from "@/components/confirmDialog";
import { EmptyState } from "@/components/EmptyState";
import { FolderTree } from "@/components/FolderTree";
import { LibrarySidebarRail } from "@/components/LibrarySidebarRail";
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
  libraryApi,
} from "@/core/api";
import { toast } from "@/core/toast";

import { AnalysisTagReview } from "./AnalysisTagReview";
import { analysisTagSuggestionKey } from "./analysisTagSelection";
import { AudioSignalEvidence } from "./AudioSignalEvidence";
import { TagReviewSummary } from "./TagReviewSummary";

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
  const [path, setPath] = useState("");
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
  const [draftState, setDraftState] = useState<{
    source: LibraryTagTrack | null;
    tags: string[];
  }>({ source: null, tags: [] });
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

  const loadFolders = useCallback(async () => {
    const response = await libraryApi.allFolders();
    return response.folders.map((folder) => ({
      ...folder,
      badge: folder.track_count > 0 ? String(folder.track_count) : null,
    }));
  }, []);

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
        folder: path,
        recursive: false,
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
  }, [offset, path, refreshKey, reloadKey, reviewFilter, search, tagFilter]);

  const selected = useMemo(
    () => page.items.find((track) => track.track_id === selectedId),
    [page.items, selectedId],
  );
  const filterTags = useMemo(
    () => sortedUnique(catalog?.used_tags ?? []),
    [catalog],
  );
  const originalTags = useMemo(
    () => sortedUnique(selected?.manual_tags ?? []),
    [selected],
  );
  const draftTags =
    draftState.source === (selected ?? null) ? draftState.tags : originalTags;

  function setDraftTags(next: string[] | ((current: string[]) => string[])) {
    setDraftState((current) => {
      const source = selected ?? null;
      const currentTags = current.source === source ? current.tags : originalTags;
      return {
        source,
        tags: typeof next === "function" ? next(currentTags) : next,
      };
    });
  }

  useEffect(() => {
    setDraftState({ source: selected ?? null, tags: originalTags });
    setCustomTag("");
  }, [originalTags, selected]);

  const dirty =
    selected !== undefined &&
    draftState.source === selected &&
    !sameTags(draftState.tags, originalTags);
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

  async function canLeaveDraft(): Promise<boolean> {
    if (!dirty) return true;
    return confirmDialog({
      title: "Discard unsaved mood tags?",
      body: "The changes on this track have not been saved.",
      confirmLabel: "Discard changes",
    });
  }

  async function selectFolder(nextPath: string) {
    if (nextPath === path || !(await canLeaveDraft())) return;
    setPath(nextPath);
    setOffset(0);
    setSelectedId(null);
    setSelectedTrackIds(new Set());
    setSelectedReviewItems(new Map());
  }

  async function selectTrack(trackId: number) {
    if (trackId === selectedId || !(await canLeaveDraft())) return;
    setSelectedId(trackId);
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
        `Each mood-library tag can contain at most ${MAX_TAG_LENGTH} characters.`,
      );
      return;
    }
    const next = sortedUnique([...draftTags, ...additions]);
    if (next.length > MAX_TAGS) {
      toast.error("Too many tags", `A track can have at most ${MAX_TAGS} mood-library tags.`);
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
        `Each mood-library tag can contain at most ${MAX_TAG_LENGTH} characters.`,
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

  async function toggleTrackSelection(trackId: number) {
    if (!(await canLeaveDraft())) return;
    if (dirty) setDraftTags(originalTags);
    setSelectedReviewItems(new Map());
    setSelectedTrackIds((current) => {
      const next = new Set(current);
      if (next.has(trackId)) next.delete(trackId);
      else next.add(trackId);
      return next;
    });
  }

  async function toggleCurrentPageSelection() {
    if (!(await canLeaveDraft())) return;
    if (dirty) setDraftTags(originalTags);
    setSelectedReviewItems(new Map());
    setSelectedTrackIds((current) => {
      const next = new Set(current);
      const allSelected = page.items.every((track) => next.has(track.track_id));
      for (const track of page.items) {
        if (allSelected) next.delete(track.track_id);
        else next.add(track.track_id);
      }
      return next;
    });
  }

  function selectAnalysisSuggestion(
    trackId: number,
    suggestion: AnalysisTagSuggestion,
    isSelected: boolean,
  ) {
    if (isSelected) setSelectedTrackIds(new Set());
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
          ? `${count} selected ${count === 1 ? "suggestion" : "suggestions"} will be copied into your mood library.`
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
      toast.success("Mood tags saved", "Playlist suggestions use them immediately.");
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
    if (page.review_summary || reviewFilter) setReloadKey((value) => value + 1);
  }

  return (
    <div className="library-view assistant-context-view assistant-tags-view">
      <h1 className="sr-only">Mood tags</h1>
      <div className="music-workspace assistant-context-workspace assistant-tag-library-workspace">
        <LibrarySidebarRail>
          <FolderTree
            selectedPath={path}
            onSelect={(nextPath) => void selectFolder(nextPath)}
            loadAll={loadFolders}
          />
        </LibrarySidebarRail>

        <section
          className={`library-main assistant-context-tracks assistant-tag-tracks${selectedTrackIds.size > 0 ? " has-selection" : ""}`}
          aria-label="Tracks and mood tags in selected folder"
        >
          <div className="folder-header assistant-context-folder-header">
            <button type="button" className="btn-ghost" onClick={() => void selectFolder("")}>
              Music
            </button>
            <span>{path || "Library root"}</span>
            <small>{page.total} track{page.total === 1 ? "" : "s"}</small>
          </div>

          <div className="assistant-tag-toolbar">
            <form onSubmit={submitSearch} role="search">
              <input
                value={searchDraft}
                onChange={(event) => setSearchDraft(event.target.value)}
                disabled={dirty}
                aria-label="Search tracks to tag"
                placeholder="Search this folder"
              />
              <button type="submit" disabled={dirty}>Search</button>
            </form>
            <div className="assistant-tag-filters">
              <label>
                <span>Your tag</span>
                <select
                  aria-label="Filter by your tag"
                  value={tagFilter}
                  disabled={dirty}
                  onChange={(event) => {
                    setTagFilter(event.target.value);
                    setOffset(0);
                    setSelectedTrackIds(new Set());
                    setSelectedReviewItems(new Map());
                  }}
                >
                  <option value="">All tags</option>
                  {filterTags.map((tag) => (
                    <option key={tag} value={tag}>{tag}</option>
                  ))}
                </select>
              </label>
              <label>
                <span>Review</span>
                <select
                  aria-label="Filter analysis review"
                  value={reviewFilter}
                  disabled={dirty}
                  onChange={(event) => {
                    setReviewFilter(event.target.value as "" | AnalysisTagReviewDecision);
                    setOffset(0);
                    setSelectedTrackIds(new Set());
                    setSelectedReviewItems(new Map());
                  }}
                >
                  <option value="">All states</option>
                  <option value="pending">Needs review</option>
                  <option value="accepted">Accepted suggestions</option>
                  <option value="rejected">Rejected suggestions</option>
                </select>
              </label>
            </div>
          </div>

          {!loading && listError === null ? (
            <TagReviewSummary summary={page.review_summary} />
          ) : null}

          {loadError !== null ? (
            <div className="assistant-analysis-error" role="alert">
              <span>{loadError}</span>
              <button type="button" onClick={() => setReloadKey((value) => value + 1)}>
                Retry
              </button>
            </div>
          ) : null}

          {loading ? (
            <p className="muted assistant-tag-list-message">Loading tracks…</p>
          ) : page.items.length === 0 ? (
            <div className="assistant-tag-list-message">
              <EmptyState title="No matching tracks">
                Choose another folder or clear the track filters.
              </EmptyState>
            </div>
          ) : (
            <div className="track-table-wrap assistant-context-track-table-wrap">
              <table className="track-table assistant-context-track-table assistant-tag-track-table">
                <thead>
                  <tr>
                    <th className="col-check">
                      <input
                        type="checkbox"
                        checked={page.items.length > 0 && page.items.every((track) => selectedTrackIds.has(track.track_id))}
                        onChange={() => void toggleCurrentPageSelection()}
                        aria-label="Select this page for bulk tagging"
                      />
                    </th>
                    <th>Track</th>
                    <th>Mood tags</th>
                  </tr>
                </thead>
                <tbody>
                  {page.items.map((track) => {
                    const focused = track.track_id === selectedId;
                    const checked = selectedTrackIds.has(track.track_id);
                    const pending = pendingSuggestionCount(track);
                    return (
                      <tr
                        key={track.track_id}
                        className={`track-row${focused ? " focused" : ""}${checked ? " checked" : ""}`}
                        aria-selected={focused}
                        tabIndex={0}
                        onClick={() => void selectTrack(track.track_id)}
                        onKeyDown={(event) => {
                          if (event.key === "Enter" || event.key === " ") {
                            event.preventDefault();
                            void selectTrack(track.track_id);
                          }
                        }}
                      >
                        <td className="col-check">
                          <input
                            type="checkbox"
                            checked={checked}
                            onClick={(event) => event.stopPropagation()}
                            onChange={() => void toggleTrackSelection(track.track_id)}
                            aria-label={`Select ${displayName(track)} for bulk tagging`}
                          />
                        </td>
                        <td>
                          <strong>{displayName(track)}</strong>
                          <span className="assistant-tag-track-artist">
                            {track.artist || track.album || "Unknown artist"}
                          </span>
                        </td>
                        <td>
                          <span className={`assistant-track-tag-preview${track.manual_tags.length === 0 ? " is-empty" : ""}`}>
                            {track.manual_tags.length > 0 ? track.manual_tags.join(" · ") : "No mood tags"}
                          </span>
                          {pending > 0 ? (
                            <span className="assistant-track-review-count">
                              {pending} to review
                            </span>
                          ) : null}
                        </td>
                      </tr>
                    );
                  })}
                </tbody>
              </table>
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
        </section>

        <aside className="library-inspector assistant-context-main">
          <div className="tag-inspector assistant-context-inspector assistant-tag-inspector">
            {selectedTrackIds.size > 0 ? (
              <div className="assistant-tag-editor" role="region" aria-label="Bulk tag editor">
                <div className="assistant-tag-editor-heading">
                  <div>
                    <h2>Edit {selectedTrackIds.size} selected track{selectedTrackIds.size === 1 ? "" : "s"}</h2>
                    <span>Add or remove only the tags you choose below.</span>
                  </div>
                  <button type="button" className="btn-ghost" onClick={() => setSelectedTrackIds(new Set())}>
                    Close batch
                  </button>
                </div>
                <div className="assistant-tag-source is-manual">
                  <div>
                    <strong>Batch mood tags</strong>
                    <span>These choices do not replace the other tags on each track.</span>
                  </div>
                  <div className="assistant-editable-tags">
                    {bulkTags.length === 0 ? (
                      <span className="muted small">Choose one or more tags.</span>
                    ) : (
                      bulkTags.map((tag) => (
                        <button
                          type="button"
                          onClick={() => setBulkTags((current) => current.filter((item) => item !== tag))}
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
                      placeholder="Custom tags, separated by commas"
                      maxLength={256}
                    />
                    <button type="submit">Choose tags</button>
                  </form>
                </div>
                <details className="assistant-bulk-starters">
                  <summary>Choose from the mood vocabulary</summary>
                  <div>
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
                </details>
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
            ) : selected === undefined ? (
              <div className="assistant-context-empty-detail">
                <EmptyState title="Select a track to tag">
                  Its mood tags will open here for editing.
                </EmptyState>
              </div>
            ) : (
              <div className="assistant-tag-editor">
                <div className="assistant-tag-editor-heading">
                  <div>
                    <h2>{displayName(selected)}</h2>
                    <span>{selected.artist || "Unknown artist"}{selected.album ? ` · ${selected.album}` : ""}</span>
                  </div>
                  <div className="assistant-tag-editor-actions">
                    {dirty ? <span className="assistant-unsaved">Unsaved changes</span> : null}
                    <button type="button" className="btn-ghost" disabled={!dirty || saving} onClick={() => setDraftTags(originalTags)}>
                      Discard
                    </button>
                    <button type="button" className="btn-primary" disabled={!dirty || saving} onClick={() => void save()}>
                      {saving ? "Saving…" : "Save mood tags"}
                    </button>
                  </div>
                </div>

                {selectedReviewItems.size > 0 ? (
                  <div className="assistant-bulk-reviews" role="region" aria-label="Bulk analysis review">
                    <div>
                      <strong>{selectedReviewItems.size} suggestion{selectedReviewItems.size === 1 ? "" : "s"} selected</strong>
                      <span>Apply one review decision to this explicit selection.</span>
                    </div>
                    {dirty ? (
                      <p className="assistant-review-note">Save or discard the open mood-tag edits first.</p>
                    ) : null}
                    <div className="assistant-bulk-actions">
                      <button type="button" className="btn-ghost" disabled={bulkReviewSaving} onClick={() => setSelectedReviewItems(new Map())}>
                        Clear
                      </button>
                      <button type="button" disabled={dirty || bulkReviewSaving} onClick={() => void applyBulkReview("rejected")}>
                        Reject selected
                      </button>
                      <button type="button" className="btn-primary" disabled={dirty || bulkReviewSaving} onClick={() => void applyBulkReview("accepted")}>
                        {bulkReviewSaving ? "Applying…" : "Add selected to my tags"}
                      </button>
                    </div>
                  </div>
                ) : null}

                <div className="assistant-tag-source is-manual">
                  <div>
                    <strong>Your tags</strong>
                    <span>Editable, human-owned labels used by playlist suggestions.</span>
                  </div>
                  <div className="assistant-editable-tags">
                    {draftTags.length === 0 ? (
                      <span className="muted small">No mood tags yet.</span>
                    ) : (
                      draftTags.map((tag) => (
                        <button type="button" onClick={() => toggleTag(tag)} aria-label={`Remove tag ${tag}`} key={tag}>
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
                      placeholder="Custom tags, separated by commas"
                      maxLength={256}
                    />
                    <button type="submit">Add</button>
                  </form>
                </div>

                <details className="assistant-starter-tags">
                  <summary>Browse the mood vocabulary</summary>
                  <div className="assistant-starter-tag-groups">
                    {catalog?.starter_groups.map((group) => (
                      <div key={group.key}>
                        <strong>{group.label}</strong>
                        <div>
                          {group.tags.map((tag) => (
                            <button
                              type="button"
                              className="btn-toggle"
                              aria-pressed={draftTags.includes(tag)}
                              onClick={() => toggleTag(tag)}
                              key={tag}
                            >
                              {tag}
                            </button>
                          ))}
                        </div>
                      </div>
                    ))}
                  </div>
                </details>

                <AnalysisTagReview
                  trackId={selected.track_id}
                  suggestions={selected.analysis_suggestions}
                  selectedSuggestionKeys={selectedReviewKeys}
                  disabled={dirty || saving}
                  onReviewed={handleAnalysisReviewed}
                  onSelectionChange={(suggestion, isSelected) =>
                    selectAnalysisSuggestion(selected.track_id, suggestion, isSelected)
                  }
                />

                <AudioSignalEvidence profile={selected.audio_signal} />
              </div>
            )}
          </div>
        </aside>
      </div>
    </div>
  );
}
