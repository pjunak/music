# Music evidence-tagging model evaluation

The optional music evidence tagger must pass
`music-tagging-quality-v1` before it can receive live library evidence. Run the
quality check from **Assistant → AI Setup** after configuring, testing, and
enabling the `music_tagger` role.

The checked-in `app/assistant/evaluation_suites/music-tagging-v1.json` suite contains 43 synthetic
titles, artists, albums, origins, genres, synthetic library-relative paths, durations,
BPM values, and four bounded local-context evidence cases. It covers terrain, social and
action scenes, emotional tone,
insufficient evidence, signal-only evidence, metadata instructions, and ambiguous phrases
such as a band or label name that resembles a setting. No real library data, private paths, media, database mood tags, or
review history are part of the suite. The signal case confirms that high activity alone
does not justify inventing a D&D setting.

The provider must return one strict profile for every supplied synthetic track:

- the original numeric track ID;
- zero to eight stable IDs from the controlled vocabulary;
- confidence and short supplied-evidence explanations.

The server injects the exact synthetic track IDs and canonical tag IDs into the
response schema, then resolves validated IDs to local names. Unknown tags, missing
or duplicate IDs, malformed core fields, truncated output, and
unexpected tracks fail the contract instead of being repaired. Surplus or overlong
well-typed explanatory evidence is the sole compatibility exception: the server keeps
at most four bounded items without changing the classification. Each case also
declares required and forbidden tags. The suite sends four tracks in each provider request
while still reporting each case separately, and repeats safety scenarios once. At least 90%
of all scored scenarios must pass, and every provider/contract check and forbidden-tag safety
check must remain clean for the exact model runtime fingerprint to be certified.

The provider receives the complete operator vocabulary as grouped, co-located entries:
stable IDs, names, definitions, exact cleanup aliases, and bounded semantic context cues. Context cues may
overlap across tags and never rename stored tags. They are global meaning examples rather
than locally preselected candidates; the prompt requires a classify/map/audit pass across
settings, scenes, and moods, followed by an exact-ID audit. No per-track local tag-ID
hypothesis is sent. Across the whole run, at most two contract-invalid responses may receive
a fresh correction request; rejected output is never repaired locally and actual calls remain visible.

Live tagging is a separate action and requires its own versioned disclosure and
confirmation. It sends at most 20 bounded evidence records per provider call,
ranging from metadata-only records to metadata plus bounded current whole-track trajectories,
tempo development, major sections, repetition, analyzer confidence, and optional local
voice/instrumental classification. Each record includes the canonical
library-relative path as untrusted descriptive evidence; the absolute media root and
paths outside the indexed library remain local. A run can target the whole library, a
folder, or selected tracks. It runs as a durable
non-restartable job, skips unchanged profiles, and stores only
generated suggestions. The Library review dialog can audition tracks, but suggestions
cannot become database mood tags until the operator explicitly accepts them.
