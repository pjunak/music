# Music evidence-tagging model evaluation

The optional music evidence tagger must pass
`music-tagging-quality-v1` before it can receive live library evidence. Run the
quality check from **Assistant → AI Setup** after configuring, testing, and
enabling the `music_tagger` role.

The checked-in `app/assistant/evaluation_suites/music-tagging-v1.json` suite contains synthetic
titles, artists, albums, origins, genres, synthetic library-relative paths, durations,
BPM values, and one bounded local-signal
evidence case. It tests clear D&D cases such
as medieval tavern dancing, dark dungeons, heroic castles, calm travel, and
insufficient evidence. No real library data, private paths, media, database mood tags, or
review history are part of the suite. The signal case confirms that high activity alone
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

Before each call, deterministic metadata matching uses the operator vocabulary's
canonical names, exact cleanup aliases, and separately editable context cues to build
high-recall candidate evidence. Context cues may overlap across tags and never rename
stored tags. The provider receives the complete compact ID/name/group index plus detailed
definitions for those candidates, rather than the full 131-tag definition catalog on
every scenario.

Live tagging is a separate action and requires its own versioned disclosure and
confirmation. It sends at most 20 bounded evidence records per provider call,
ranging from metadata-only records to metadata plus bounded current local energy,
brightness, tension, tempo, and confidence values. Each record includes the canonical
library-relative path as untrusted descriptive evidence; the absolute media root and
paths outside the indexed library remain local. A run can target the whole library, a
folder, or selected tracks. It runs as a durable
non-restartable job, skips unchanged profiles, and stores only
generated suggestions. The Library review dialog can audition tracks, but suggestions
cannot become database mood tags until the operator explicitly accepts them.
