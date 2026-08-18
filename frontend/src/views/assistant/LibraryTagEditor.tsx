import { type FormEvent, useEffect, useMemo, useState } from "react";

import { EmptyState } from "@/components/EmptyState";
import {
  type LibraryTagPage,
  type LibraryTagTrack,
  type ManualTagCatalog,
  assistantApi,
} from "@/core/api";
import { toast } from "@/core/toast";

const PAGE_SIZE = 50;
const MAX_TAGS = 32;
const MAX_TAG_LENGTH = 64;

function displayName(track: LibraryTagTrack): string {
  return track.display_title || track.title || track.path;
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

export function LibraryTagEditor() {
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
  const [offset, setOffset] = useState(0);
  const [selectedId, setSelectedId] = useState<number | null>(null);
  const [draftTags, setDraftTags] = useState<string[]>([]);
  const [customTag, setCustomTag] = useState("");
  const [loading, setLoading] = useState(true);
  const [saving, setSaving] = useState(false);
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
  }, [reloadKey]);

  useEffect(() => {
    let disposed = false;
    setLoading(true);
    void assistantApi
      .listLibraryTags({
        ...(search ? { search } : {}),
        ...(tagFilter ? { tag: tagFilter } : {}),
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
  }, [offset, reloadKey, search, tagFilter]);

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

  function submitSearch(event: FormEvent) {
    event.preventDefault();
    setOffset(0);
    setSearch(searchDraft.trim());
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

  return (
    <section className="surface-card assistant-tag-workspace">
      <div className="assistant-section-heading">
        <div>
          <p className="assistant-eyebrow">Human-owned library context</p>
          <h2>Manual playlist tags</h2>
          <p>
            Add your own D&amp;D settings, scenes, and moods. These labels stay
            separate from analysis or future AI suggestions.
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
      </div>

      {loadError !== null ? (
        <div className="assistant-analysis-error" role="alert">
          <span>{loadError}</span>
          <button type="button" onClick={() => setReloadKey((value) => value + 1)}>
            Retry
          </button>
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
            <div className="assistant-tag-track-list" role="listbox" aria-label="Tracks">
              {page.items.map((track) => (
                <button
                  type="button"
                  role="option"
                  aria-selected={track.track_id === selectedId}
                  className={track.track_id === selectedId ? "is-selected" : ""}
                  onClick={() => setSelectedId(track.track_id)}
                  key={track.track_id}
                >
                  <strong>{displayName(track)}</strong>
                  <span>{track.artist || track.album || track.path}</span>
                  <span className="assistant-track-tag-preview">
                    {track.manual_tags.length > 0
                      ? track.manual_tags.join(" · ")
                      : "No manual tags"}
                  </span>
                </button>
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

              <div className="assistant-tag-source is-analysis">
                <div>
                  <strong>Analysis / AI tags</strong>
                  <span>
                    Read-only output from {selected.analysis_analyzer ?? "no analyzer yet"};
                    rerunning analysis never changes your tags.
                  </span>
                </div>
                <div className="assistant-readonly-tags">
                  {selected.analysis_tags.length > 0 ? (
                    selected.analysis_tags.map((tag) => <span key={tag}>{tag}</span>)
                  ) : (
                    <span className="muted small">No analysis tags available.</span>
                  )}
                </div>
              </div>

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
    </section>
  );
}
