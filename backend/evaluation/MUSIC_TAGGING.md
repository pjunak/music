# Music evidence-tagging model evaluation

The optional music evidence tagger must pass
`music-tagging-quality-v1` before it can receive live library evidence. Run the
quality check from **Assistant → AI Setup** after configuring, testing, and
enabling the `music_tagger` role.

The checked-in `app/assistant/evaluation_suites/music-tagging-v1.json` suite contains synthetic
titles, artists, albums, origins, genres, durations, BPM values, and one bounded local-signal
evidence case. It tests clear D&D cases such
as medieval tavern dancing, dark dungeons, heroic castles, calm travel, and
insufficient evidence. No real library data, paths, media, manual tags, or review
history are part of the suite. The signal case confirms that high activity alone
does not justify inventing a D&D setting.

The provider must return one strict profile for every supplied synthetic track:

- the original numeric track ID;
- zero to eight stable IDs from the controlled vocabulary;
- bounded energy, brightness, and tension axes;
- confidence and short supplied-evidence explanations.

The server injects the exact synthetic track IDs and canonical tag IDs into the
response schema, then resolves validated IDs to local names. Unknown tags, missing
or duplicate IDs, malformed fields, truncated output, and
unexpected tracks fail the contract instead of being repaired. Each case also
declares required and forbidden tags. All cases must pass for the exact model
runtime fingerprint to be certified.

Live tagging is a separate action and requires its own versioned disclosure and
confirmation. It sends at most 20 path-free evidence records per provider call,
ranging from metadata-only records to metadata plus bounded current local energy,
brightness, tension, tempo, and confidence values. It runs as a durable
non-restartable job, skips unchanged profiles, and stores only
generated suggestions. Suggestions cannot become manual tags until the operator
accepts them in the existing tag-review workspace.
