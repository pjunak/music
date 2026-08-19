# Music tagging model evaluation

The optional metadata music tagger must pass
`music-tagging-quality-v1` before it can receive live library metadata. Run the
quality check from **Assistant → AI Setup** after configuring, testing, and
enabling the `music_tagger` role.

The checked-in `music-tagging-v1.json` suite contains synthetic titles, artists,
albums, origins, genres, durations, and BPM values. It tests clear D&D cases such
as medieval tavern dancing, dark dungeons, heroic castles, calm travel, and
insufficient metadata. No real library data, paths, media, manual tags, or review
history are part of the suite.

The provider must return one strict profile for every supplied synthetic track:

- the original numeric track ID;
- zero to eight tags from the fixed D&D starter vocabulary;
- bounded energy, brightness, and tension axes;
- confidence and short metadata-based evidence.

Unknown tags, missing or duplicate IDs, malformed fields, truncated output, and
unexpected tracks fail the contract instead of being repaired. Each case also
declares required and forbidden tags. All cases must pass for the exact model
runtime fingerprint to be certified.

Live tagging is a separate action and requires its own versioned disclosure and
confirmation. It sends at most 20 path-free metadata records per provider call,
runs as a durable non-restartable job, skips unchanged profiles, and stores only
generated suggestions. Suggestions cannot become manual tags until the operator
accepts them in the existing tag-review workspace.
