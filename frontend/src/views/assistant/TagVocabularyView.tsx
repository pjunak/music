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
import { AssistantInfoPopover } from "./AssistantInfoPopover";
import { TagCatalogManager } from "./TagCatalogManager";

type TermListField = "aliases" | "context_cues";
type TermDrafts = Record<string, Record<TermListField, string>>;

function cloneGroups(groups: TagVocabularyGroup[]): TagVocabularyGroup[] {
  return groups.map((group) => ({
    ...group,
    tags: group.tags.map((tag) => ({
      ...tag,
      aliases: [...tag.aliases],
      context_cues: [...tag.context_cues],
    })),
  }));
}

function draftTerms(groups: TagVocabularyGroup[]): TermDrafts {
  return Object.fromEntries(
    groups.flatMap((group) =>
      group.tags.map((tag) => [
        tag.id,
        {
          aliases: tag.aliases.join(", "),
          context_cues: tag.context_cues.join(", "),
        },
      ]),
    ),
  );
}

function parseTerms(value: string): string[] {
  return value
    .split(",")
    .map((item) => item.trim())
    .filter(Boolean);
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
  const [termDrafts, setTermDrafts] = useState<TermDrafts>({});

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
        setTermDrafts(draftTerms(nextVocabulary.groups));
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

  function updateTerms(
    groupKey: string,
    tag: TagVocabularyEntry,
    field: TermListField,
    value: string,
  ) {
    setTermDrafts((current) => ({
      ...current,
      [tag.id]: {
        aliases: current[tag.id]?.aliases ?? tag.aliases.join(", "),
        context_cues:
          current[tag.id]?.context_cues ?? tag.context_cues.join(", "),
        [field]: value,
      },
    }));
    updateTag(groupKey, tag.id, (item) => ({
      ...item,
      [field]: parseTerms(value),
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
    setTermDrafts((current) => ({
      ...current,
      [id]: { aliases: "", context_cues: "" },
    }));
    updateGroup(groupKey, (item) => ({
      ...item,
      tags: [
        ...item.tags,
        {
          id,
          name,
          description: `Describe the precise meaning of ${name}.`,
          aliases: [],
          context_cues: [],
        },
      ],
    }));
  }

  function promoteUsedTag(name: string) {
    const groupKey = targetGroups[name] ?? groups[0]?.key;
    const group = groups.find((item) => item.key === groupKey);
    if (group === undefined) return;
    const id = newTagId(group, name, groups);
    setTermDrafts((current) => ({
      ...current,
      [id]: { aliases: "", context_cues: "" },
    }));
    updateGroup(group.key, (item) => ({
      ...item,
      tags: [
        ...item.tags,
        {
          id,
          name,
          description: `Describe the precise meaning of ${name}.`,
          aliases: [],
          context_cues: [],
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
      setTermDrafts(draftTerms(saved.groups));
      toast.success(
        "Mood vocabulary saved",
        "New model runs will use this exact revision; older suggestions are now stale.",
      );
      refresh();
    } catch (error) {
      toast.error("Mood vocabulary was not saved", errorMessage(error));
      if (error instanceof ApiError && error.status === 409) refresh();
    } finally {
      setSaving(false);
    }
  }

  return (
    <div className="assistant-vocabulary-view">
      <header className="assistant-page-header assistant-vocabulary-header">
        <div>
          <p className="assistant-eyebrow">Database-only music context</p>
          <h1>Mood tag vocabulary</h1>
          <p>Define the exact terrain, scene, and mood choices used by tagging.</p>
        </div>
        <div className="assistant-vocabulary-header-tools">
          <AssistantInfoPopover label="How it works" title="A controlled vocabulary">
            <p>
              These tags live only in the music database. They are separate from album,
              year, genre, and other metadata embedded in audio files.
            </p>
            <p>
              Models choose exact IDs, the server rejects unknown output, and every
              suggestion still requires review. Aliases drive deterministic cleanup;
              context cues only help highlight likely choices.
            </p>
          </AssistantInfoPopover>
          <div className="assistant-vocabulary-summary" aria-label="Vocabulary summary">
            <strong>{tagCount}</strong>
            <span>canonical tags</span>
            <small>revision {vocabulary?.revision ?? "—"}</small>
          </div>
        </div>
      </header>

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
                <div className="assistant-vocabulary-table-wrap">
                  <table className="assistant-vocabulary-table">
                    <thead>
                      <tr>
                        <th scope="col">Canonical tag</th>
                        <th scope="col">Selection meaning</th>
                        <th scope="col">Exact aliases</th>
                        <th scope="col">Context cues</th>
                        <th scope="col"><span className="sr-only">Actions</span></th>
                      </tr>
                    </thead>
                    <tbody>
                      {group.tags.map((tag) => (
                        <tr key={tag.id}>
                          <th scope="row">
                            <input
                              aria-label={`Canonical name for ${tag.id}`}
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
                            <code title={tag.id}>{tag.id}</code>
                          </th>
                          <td>
                            <input
                              aria-label={`Selection meaning for ${tag.name}`}
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
                          </td>
                          <td>
                            <input
                              aria-label={`Exact aliases for ${tag.name}`}
                              value={termDrafts[tag.id]?.aliases ?? tag.aliases.join(", ")}
                              placeholder="inn, pub, alehouse"
                              disabled={saving}
                              onChange={(event) =>
                                updateTerms(group.key, tag, "aliases", event.target.value)
                              }
                            />
                          </td>
                          <td>
                            <input
                              aria-label={`Context cues for ${tag.name}`}
                              title="Cues may overlap. They highlight candidates but never rename library tags."
                              value={
                                termDrafts[tag.id]?.context_cues ??
                                tag.context_cues.join(", ")
                              }
                              placeholder="dance, jig, banquet"
                              disabled={saving}
                              onChange={(event) =>
                                updateTerms(group.key, tag, "context_cues", event.target.value)
                              }
                            />
                          </td>
                          <td className="assistant-vocabulary-table-action">
                            <button
                              type="button"
                              className="btn-ghost"
                              disabled={saving}
                              onClick={() =>
                                updateGroup(group.key, (item) => ({
                                  ...item,
                                  tags: item.tags.filter(
                                    (candidate) => candidate.id !== tag.id,
                                  ),
                                }))
                              }
                              aria-label={`Remove ${tag.name} from the vocabulary`}
                            >
                              Remove
                            </button>
                          </td>
                        </tr>
                      ))}
                    </tbody>
                  </table>
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
                    These database mood tags remain valid operator data, but models cannot
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
                onClick={() => {
                  setGroups(cloneGroups(vocabulary.groups));
                  setTermDrafts(draftTerms(vocabulary.groups));
                }}
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
