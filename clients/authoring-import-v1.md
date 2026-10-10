# Authoring import document v1

`authoring-import/v1` is the external, import-only contract for preparing Authoring resources in a
tool or language model and reviewing them in the web app. The app accepts a `.json` file or pasted
JSON. Nothing is written during preview, and the final commit only creates the items the operator
selects.

The server is the authority for validation. Authenticated tools can retrieve the exact JSON Schema
from `GET /api/authoring/import/document/schema`.

## Example

```json
{
  "schema": "authoring-import/v1",
  "name": "Rainy night draft",
  "playlists": [
    {
      "name": "Rainy Night Walk",
      "category": "exploration",
      "tracks": [
        "Soundtracks/City/Neon Rain.flac",
        "Soundtracks/City/Empty Streets.flac"
      ]
    }
  ],
  "soundboards": [
    {
      "id": "city-rain",
      "name": "City rain",
      "categories": [
        {
          "id": "weather",
          "name": "Weather",
          "items": [
            {
              "file": "SFX/Weather/Distant Thunder.ogg",
              "name": "Distant thunder",
              "icon": "⚡"
            }
          ]
        }
      ]
    }
  ],
  "interrupts": [
    {
      "name": "Sudden pursuit",
      "playlist": "Rainy Night Walk",
      "fade_in_ms": 800,
      "fade_out_ms": 1200,
      "return_to_ambient": true
    }
  ],
  "presets": [
    {
      "id": "rainy-alley",
      "name": "Rainy alley",
      "description": "Dark, distant and reflective.",
      "effects": [
        { "type": "lowpass", "frequency": 9000 },
        { "type": "reverb", "wet": 0.28 }
      ],
      "crossfade_ms": 1800
    }
  ],
  "cues": [
    {
      "id": "enter-rainy-city",
      "name": "Enter the rainy city",
      "preset": "rainy-alley",
      "playlist": "Rainy Night Walk",
      "sfx": [
        {
          "soundboard": "city-rain",
          "item": "SFX/Weather/Distant Thunder.ogg",
          "volume": 0.7
        }
      ]
    }
  ]
}
```

Every top-level resource array is optional, but the document must contain at least one resource.
Unknown fields are rejected so a misspelling cannot be silently ignored.

Optional fields not shown above:

- soundboard item `hotkey` (at most 16 characters);
- interrupt `soundboard_item` (instead of `playlist`) and `duck_to` (`0..1`);
- cue `description` (at most 2,000 characters), `start_index` (at most 100,000) and `start_ms`;
- cue `loops`: `{ "soundboard", "item", "interval_s", "volume" }` entries that repeat a sound
  every `interval_s` seconds (`1..3600`); `volume` defaults to 1 as for cue `sfx`.

The generated OpenAPI schema for the preview endpoint is the exact field reference.

## Reference rules

- IDs use lowercase letters, digits, hyphens, and underscores, start with a letter or digit, and
  are at most 64 characters.
- Playlist tracks are canonical paths relative to the music library; soundboard item files and
  the cue/interrupt references to them are relative to the SFX library. Use forward slashes;
  absolute paths, backslashes, drive letters, `.` and `..` segments are invalid.
- Missing playlist tracks are warnings. The playlist can still be selected, and unavailable tracks
  are listed and omitted during commit.
- An interrupt references exactly one source: `playlist` or `soundboard_item`.
- Cue `preset` values reference preset IDs. Cue and interrupt `playlist` values reference playlist
  names. Cue sound entries reference a soundboard ID and an item `file` path.
- When a referenced resource is part of the same document, both it and the dependent cue or
  interrupt must be selected. A reference already satisfied by the target mode needs no additional
  selection.
- Supported effect types in v1 are `eq`, `reverb`, `lowpass`, `highpass`, `bandpass`, `delay`,
  `distortion`, and `tremolo`. Effect-specific parameter validation will become stricter in a later
  schema version; unknown effect types are already rejected during preview.

## Review and commit API

All endpoints require an authenticated Authoring session.

1. `POST /api/authoring/import/document/preview` with `target_mode_id`, optional `source_name`, and
   `document` returns every candidate with `ready`, `conflict`, or `invalid` status and a per-item
   issue list.
2. Present that response to the operator and let them choose from the `ready` items.
3. `POST /api/authoring/import/document/commit` with the same document plus an `items` array of
   `{ "kind", "resource_id" }` selections.

Commit repeats the complete preview while holding the import lock. A target collision that appears
after the original review is skipped rather than overwritten. Playlist rows, mode files, and the
mode manifest are applied as one operation; if validation or mode reload fails, all staged changes
are rolled back.

The current safety limits are 1 MiB per file or paste in the web app, 500 total resources, 20,000
playlist track references, 20,000 soundboard items, and 20,000 cue sound actions (`sfx` plus
`loops`) per document; 10,000 tracks per playlist; 500 `sfx` and 500 `loops` per cue; and 32
effects with `crossfade_ms` at most 60,000 per preset.
