import { useCallback, useEffect, useMemo, useState } from "react";

import { inputDialog } from "@/components/inputDialog";
import type {
  ManualTagCatalog,
  TagVocabulary,
  TagVocabularyEntry,
  TagVocabularyGroup,
} from "@/core/api";
import { ApiError, assistantApi } from "@/core/api";
import { toast } from "@/core/toast";

import { ModelTagCleanupPanel } from "./ModelTagCleanupPanel";
import { TagCatalogManager } from "./TagCatalogManager";

function cloneGroups(groups: TagVocabularyGroup[]): TagVocabularyGroup[] {
  return groups.map((group) => ({
    ...group,
    tags: group.tags.map((tag) => ({ ...tag, aliases: [...tag.aliases] })),
  }));
}

function tagSlug(value: string): string {
  return value
    .normalize("NFKD")
    .toLowerCase()
    .replace(/[^a-z0-9]+/g, "-")
    .replace(/^-+|-+$/g, "")
    .slice(0, 40);
}

function normalizedTagName(value: string): string {
  return value.normalize("NFKC").trim().replace(/\s+/g, " ").toLowerCase();
}

function newTagId(
  group: TagVocabularyGroup,
  name: string,
  groups: TagVocabularyGroup[],
): string {
  const used = new Set(groups.flatMap((item) => item.tags.map((tag) => tag.id)));
  const stem = `${group.key}.${tagSlug(name) || "new-tag"}`;
  if (!used.has(stem)) return stem;
  let suffix = 2;
  while (used.has(`${stem}-${suffix}`)) suffix += 1;
  return `${stem}-${suffix}`;
}

function errorMessage(error: unknown): string {
  return error instanceof Error ? error.message : "The vocabulary is unavailable.";
}

export function TagVocabularyView() {
  const [vocabulary, setVocabulary] = useState<TagVocabulary | null>(null);
  const [catalog, setCatalog] = useState<ManualTagCatalog | null>(null);
  const [groups, setGroups] = useState<TagVocabularyGroup[]>([]);
  const [loading, setLoading] = useState(true);
  const [saving, setSaving] = useState(false);
  const [loadError, setLoadError] = useState<string | null>(null);
  const [refreshKey, setRefreshKey] = useState(0);
  const [targetGroups, setTargetGroups] = useState<Record<string, string>>({});

  const refresh = useCallback(() => setRefreshKey((value) => value + 1), []);

  useEffect(() => {
    let disposed = false;
    setLoading(true);
    Promise.all([
      assistantApi.getTagVocabulary(),
      assistantApi.getManualTagCatalog(),
    ])
      .then(([nextVocabulary, nextCatalog]) => {
        if (disposed) return;
        setVocabulary(nextVocabulary);
        setGroups(cloneGroups(nextVocabulary.groups));
        setCatalog(nextCatalog);
        setTargetGroups({});
        setLoadError(null);
      })
      .catch((error: unknown) => {
        if (!disposed) setLoadError(errorMessage(error));
      })
      .finally(() => {
        if (!disposed) setLoading(false);
      });
    return () => {
      disposed = true;
    };
  }, [refreshKey]);

  const canonicalNames = useMemo(
    () => new Set(groups.flatMap((group) => group.tags.map((tag) => tag.name))),
    [groups],
  );
  const outsideVocabulary = useMemo(
    () => catalog?.tag_usage.filter((item) => !canonicalNames.has(item.tag)) ?? [],
    [canonicalNames, catalog],
  );
  const dirty =
    vocabulary !== null && JSON.stringify(groups) !== JSON.stringify(vocabulary.groups);
  const tagCount = groups.reduce((total, group) => total + group.tags.length, 0);

  function updateGroup(
    groupKey: string,
    update: (group: TagVocabularyGroup) => TagVocabularyGroup,
  ) {
    setGroups((current) =>
      current.map((group) => (group.key === groupKey ? update(group) : group)),
    );
  }

  function updateTag(
    groupKey: string,
    tagId: string,
    update: (tag: TagVocabularyEntry) => TagVocabularyEntry,
  ) {
    updateGroup(groupKey, (group) => ({
      ...group,
      tags: group.tags.map((tag) => (tag.id === tagId ? update(tag) : tag)),
    }));
  }

  async function addTag(groupKey: string) {
    const group = groups.find((item) => item.key === groupKey);
    if (group === undefined) return;
    const entered = await inputDialog({
      title: `Add ${group.label.toLowerCase()} tag`,
      body: "Give the canonical choice a short name. You can define its precise meaning and aliases next.",
      label: "Canonical tag name",
      confirmLabel: "Add tag",
      validate: (value) => {
        const name = normalizedTagName(value);
        if (!name) return "Enter a tag name.";
        if (name.length > 64) return "Use at most 64 characters.";
        if (canonicalNames.has(name)) return "That canonical tag already exists.";
        return null;
      },
    });
    if (entered === null) return;
    const name = normalizedTagName(entered);
    const id = newTagId(group, name, groups);
    updateGroup(groupKey, (item) => ({
      ...item,
      tags: [
        ...item.tags,
        {
          id,
          name,
          description: `Describe the precise meaning of ${name}.`,
          aliases: [],
        },
      ],
    }));
  }

  function promoteUsedTag(name: string) {
    const groupKey = targetGroups[name] ?? groups[0]?.key;
    const group = groups.find((item) => item.key === groupKey);
    if (group === undefined) return;
    updateGroup(group.key, (item) => ({
      ...item,
      tags: [
        ...item.tags,
        {
          id: newTagId(group, name, groups),
          name,
          description: `Describe the precise meaning of ${name}.`,
          aliases: [],
        },
      ],
    }));
  }

  async function save() {
    if (vocabulary === null || !dirty) return;
    setSaving(true);
    try {
      const saved = await assistantApi.updateTagVocabulary(
        vocabulary.revision,
        groups,
      );
      setVocabulary(saved);
      setGroups(cloneGroups(saved.groups));
      toast.success(
        "Tag vocabulary saved",
        "New model runs will use this exact revision; older suggestions are now stale.",
      );
      refresh();
    } catch (error) {
      toast.error("Tag vocabulary was not saved", errorMessage(error));
      if (error instanceof ApiError && error.status === 409) refresh();
    } finally {
      setSaving(false);
    }
  }

  return (
    <div className="assistant-vocabulary-view">
      <header className="assistant-page-header assistant-vocabulary-header">
        <div>
          <p className="assistant-eyebrow">Server-owned choices</p>
          <h1>Tag vocabulary</h1>
          <p>
            Define the only tags the connected models may choose. Stable IDs travel
            through the AI pipeline; these names and descriptions are restored only
            after the response passes strict validation.
          </p>
        </div>
        <div className="assistant-vocabulary-summary" aria-label="Vocabulary summary">
          <strong>{tagCount}</strong>
          <span>canonical tags</span>
          <small>revision {vocabulary?.revision ?? "—"}</small>
        </div>
      </header>

      <section className="surface-card assistant-vocabulary-contract">
        <div>
          <span>1</span>
          <strong>You define</strong>
          <small>names, meanings, aliases</small>
        </div>
        <div>
          <span>2</span>
          <strong>Model chooses IDs</strong>
          <small>no free-form tag output</small>
        </div>
        <div>
          <span>3</span>
          <strong>Server validates</strong>
          <small>unknown or incomplete output fails</small>
        </div>
        <div>
          <span>4</span>
          <strong>You review</strong>
          <small>nothing silently edits the library</small>
        </div>
      </section>

      {loadError !== null ? (
        <div className="assistant-analysis-error" role="alert">
          <span>{loadError}</span>
          <button type="button" onClick={refresh}>Retry</button>
        </div>
      ) : null}
      {loading && vocabulary === null ? <p className="muted">Loading vocabulary…</p> : null}

      {vocabulary !== null ? (
        <form
          className="assistant-vocabulary-editor"
          onSubmit={(event) => {
            event.preventDefault();
            void save();
          }}
        >
          <div className="assistant-vocabulary-groups">
            {groups.map((group) => (
              <section className="surface-card assistant-vocabulary-group" key={group.key}>
                <div className="assistant-vocabulary-group-heading">
                  <div>
                    <p className="assistant-eyebrow">{group.key}</p>
                    <h2>{group.label}</h2>
                    <p>{group.description}</p>
                  </div>
                  <span>{group.tags.length} tags</span>
                </div>
                <div className="assistant-vocabulary-tags">
                  {group.tags.map((tag) => (
                    <div className="assistant-vocabulary-tag" key={tag.id}>
                      <div className="assistant-vocabulary-tag-id">
                        <code>{tag.id}</code>
                        <button
                          type="button"
                          className="btn-ghost"
                          disabled={saving}
                          onClick={() =>
                            updateGroup(group.key, (item) => ({
                              ...item,
                              tags: item.tags.filter((candidate) => candidate.id !== tag.id),
                            }))
                          }
                          aria-label={`Remove ${tag.name} from the vocabulary`}
                        >
                          Remove
                        </button>
                      </div>
                      <label className="field">
                        <span className="field-label">Canonical name</span>
                        <input
                          value={tag.name}
                          maxLength={64}
                          required
                          disabled={saving}
                          onChange={(event) =>
                            updateTag(group.key, tag.id, (item) => ({
                              ...item,
                              name: event.target.value,
                            }))
                          }
                        />
                      </label>
                      <label className="field assistant-vocabulary-description">
                        <span className="field-label">Selection meaning</span>
                        <input
                          value={tag.description}
                          maxLength={300}
                          required
                          disabled={saving}
                          onChange={(event) =>
                            updateTag(group.key, tag.id, (item) => ({
                              ...item,
                              description: event.target.value,
                            }))
                          }
                        />
                      </label>
                      <label className="field assistant-vocabulary-aliases">
                        <span className="field-label">Exact aliases (comma-separated)</span>
                        <input
                          value={tag.aliases.join(", ")}
                          placeholder="inn, pub, alehouse"
                          disabled={saving}
                          onChange={(event) =>
                            updateTag(group.key, tag.id, (item) => ({
                              ...item,
                              aliases: event.target.value
                                .split(",")
                                .map((value) => value.trim())
                                .filter(Boolean),
                            }))
                          }
                        />
                      </label>
                    </div>
                  ))}
                </div>
                <button
                  type="button"
                  className="btn-secondary assistant-vocabulary-add"
                  disabled={saving}
                  onClick={() => void addTag(group.key)}
                >
                  Add {group.label.toLowerCase()} tag
                </button>
              </section>
            ))}
          </div>

          {outsideVocabulary.length > 0 ? (
            <section className="surface-card assistant-vocabulary-outside">
              <div className="assistant-section-heading">
                <div>
                  <p className="assistant-eyebrow">Library exceptions</p>
                  <h2>Used outside the vocabulary</h2>
                  <p>
                    These manual tags remain valid operator data, but models cannot
                    generate them until you promote them to a canonical group.
                  </p>
                </div>
                <span>{outsideVocabulary.length}</span>
              </div>
              <div className="assistant-vocabulary-outside-list">
                {outsideVocabulary.map((item) => (
                  <div key={item.tag}>
                    <span>
                      <strong>{item.tag}</strong>
                      <small>{item.track_count} track{item.track_count === 1 ? "" : "s"}</small>
                    </span>
                    <select
                      aria-label={`Group for ${item.tag}`}
                      value={targetGroups[item.tag] ?? groups[0]?.key ?? ""}
                      onChange={(event) =>
                        setTargetGroups((current) => ({
                          ...current,
                          [item.tag]: event.target.value,
                        }))
                      }
                    >
                      {groups.map((group) => (
                        <option value={group.key} key={group.key}>{group.label}</option>
                      ))}
                    </select>
                    <button
                      type="button"
                      className="btn-secondary"
                      disabled={saving || canonicalNames.has(item.tag)}
                      onClick={() => promoteUsedTag(item.tag)}
                    >
                      Add to vocabulary
                    </button>
                  </div>
                ))}
              </div>
            </section>
          ) : null}

          <div className="assistant-vocabulary-savebar">
            <span>
              {dirty
                ? "Unsaved vocabulary changes"
                : "The saved vocabulary is active for new AI runs."}
            </span>
            {dirty ? (
              <button
                type="button"
                className="btn-ghost"
                disabled={saving}
                onClick={() => setGroups(cloneGroups(vocabulary.groups))}
              >
                Discard changes
              </button>
            ) : null}
            <button className="btn-primary" type="submit" disabled={!dirty || saving}>
              {saving ? "Saving…" : "Save vocabulary"}
            </button>
          </div>
        </form>
      ) : null}

      <section className="surface-card assistant-vocabulary-library-tools">
        <div className="assistant-section-heading">
          <div>
            <p className="assistant-eyebrow">Operator-owned catalog</p>
            <h2>Used tag maintenance</h2>
            <p>
              Rename or merge tags already attached to tracks, then use deterministic
              aliases and spelling rules before involving a model.
            </p>
          </div>
        </div>
        <TagCatalogManager catalog={catalog} onChanged={refresh} />
      </section>

      <ModelTagCleanupPanel onCatalogChanged={refresh} />
    </div>
  );
}
