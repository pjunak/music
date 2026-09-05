# Stylesheet ownership

`global.css` is the single eager entry point, imported by `main.tsx`. Its ordered
imports preserve the existing cascade and avoid styles depending on which route
was visited first. Keep the order explicit; importing these files from lazy views
would change loading and override behavior.

| File | Owner |
|---|---|
| `base-and-components.css` | Tokens, base elements, reusable controls, and established shell/library/editor components. |
| `authoring-import.css` | Authoring import preview and selection dialog. |
| `assistant.css` | Assistant tools and provider/model setup. |
| `shell-and-responsive.css` | Settings, playback footer, modals, and shared responsive/touch overrides. |
| `library-cleanup.css` | Cleanup dialog and Assistant cleanup workspace. |
| `mood-library.css` | Mood-tagging workflow. |
| `track-context.css` | Local context browser and its responsive layout. |

Add feature-specific rules to their owner. Shared tokens and reusable component
rules stay shared. This is file ownership, not CSS selector isolation; existing
selectors and their order remain part of the contract. Further extraction should
follow actual feature work rather than delay it.

The initial extraction preserved every declaration and rule in the original order;
the production CSS was byte-for-byte identical before and after the split.
