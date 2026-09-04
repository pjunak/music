# Assistant and Authoring UX philosophy

The Assistant is an optional drafting layer around the product's existing local
tools. Authoring remains the place where playlists, presets, cues, soundboards,
and interrupts become operator-owned resources.

## Core interaction

Use one consistent lifecycle:

1. **Configure** the goal and local constraints.
2. **Generate** a bounded draft, locally or through an explicitly enabled model.
3. **Review** the proposed tracks, tags, or values.
4. **Create** through the existing validated Authoring transaction.
5. **Tune** the resulting normal resource in its ordinary editor.

Generated output never receives its own permanent editor or write path. An
Assistant embedded in Authoring is a sidecar around the same workflow used by
the standalone Assistant page; it is not a second implementation.

## Information architecture

The Assistant navigation is organized around five operator goals:

- **Playlist Builder** for local or model-refined playlist drafts.
- **EQ Assistant** for bounded graphic-EQ starting points.
- **Mood Library** for analysis, factual context, mood tagging, review, and the
  controlled mood vocabulary.
- **Library cleanup** for filename, folder, and embedded-metadata repair, catalog
  source policy, optional model use, and rollback history.
- **AI setup** for provider connections, model routing, request limits, and model tests.

Reusable provider connections and task-model routing belong under AI setup. Each task
workspace decides whether to use an available model; it does not duplicate provider or
request configuration. Source policy stays beside Library cleanup, while vocabulary,
analysis, and tagging stay together because they share one evidence and review pipeline.
Playlist and EQ remain distinct because their inputs, output editors, and listening
workflows are materially different.

Library cleanup uses one canonical workspace with three tabs: **Clean up**,
**Sources**, and **History & rollback**. Its planned model role stays visible in AI
setup instead of creating a second configuration surface. The Library toolbar is
a scoped shortcut into that same workspace, carrying the current folder or selected
tracks. Cleanup history is not general Assistant activity: it owns downloadable
change journals and executable rollback, so it stays with the cleanup tool.

## Placement and width

- Playlist and EQ drafting may appear as an optional sidecar inside their
  corresponding Authoring editor. Keep the normal list visible so the operator
  understands where the result will live.
- Evidence browsers, vocabularies, and routing tables use the full available
  width because comparison is their primary job.
- Provider creation and destructive maintenance stay constrained and collapsed
  until requested.
- Long diagnostics use persistent disclosures or drawers. Hover information is
  limited to short definitions and must also work with keyboard focus and click.

The sidecar is marked by one teal signal line and sparkle marker. This is the
single visual signature for optional assistance; status colors remain reserved
for success, warning, and failure.

## Language

Actions use the same verbs across tasks: **Generate**, **Review**, **Create**,
and **Continue editing**. User-facing copy describes the resource being made,
not the import schema or provider implementation. Technical contracts and
algorithm versions remain available as supporting information.

## Safety and ownership

- Preserve every task's existing provider disclosure, quality gate, and local
  validation.
- Use Authoring's preview/select/commit path for generated playlists and
  presets. Never write directly from a model response.
- After creation, open the ordinary Authoring editor. From that point onward the
  resource behaves exactly like one created manually.
- Mood suggestions remain database-only and require explicit review. They do
  not become embedded file metadata.
- Cleanup may change paths, filenames, and embedded metadata only after one
  grouped review. Applied changes use the server journal and remain downloadable
  and rollback-safe from the cleanup workspace.
- Do not create a second frontend store for lasting authored or playback state.
  Server responses remain authoritative.

## Extension rule

Embed an Assistant in another editor only when its output can become a normal
resource through an existing validated transaction and the ordinary editor can
immediately refine every material field. Otherwise keep the tool in the
standalone Assistant workspace until that boundary exists.
