# Dates and composer credits

Music reads, indexes, searches and edits three file-backed strings: `release_date`,
`original_release_date` and `composer`. The library shows them below track titles,
and the tag inspector offers single/selection edits. Empty/null clears a field; omitted
fields retain each file's original value. Dates accept real calendar values at year, month
or day precision. Original release date refers to the album's first release, not the
recording date or an earlier unrelated compilation. Composer credits are separate from
performing and album artists. Multiple names are displayed as a semicolon-separated credit;
Music does not infer person identities by splitting that text.

Schema 14 adds the indexed fields after the normal verified backup. Existing numeric years
backfill year-only release dates. Rescan after deployment to load full dates and composer
credits already present in files. Rescanning is read-only for audio files. Older clients can
continue reading numeric `year`. A year-only edit preserves a matching precise date and
rejects changing/clearing it unless `release_date` is supplied explicitly. Supplying both
fields with inconsistent years is rejected. An explicit date edit updates numeric year on
verified readback; it does not modify original release date.

## File mappings

| Container | Release date | Original release date | Composer |
|---|---|---|---|
| ID3 (MP3, AIFF and WAV primary tags) | TDRC | TDOR | TCOM |
| Vorbis comments (FLAC, Ogg, Opus) | DATE | ORIGINALDATE | COMPOSER |
| MP4/M4A | ©day | iTunes freeform ORIGINALDATE | ©wrt |
| ASF/WMA through FFmpeg | date / WM/Year | WM/OriginalReleaseTime | composer / WM/Composer |

Mappings follow [Picard's tag mapping reference](https://picard-docs.musicbrainz.org/en/latest/appendices/tag_mapping.html)
and the installed Lofty 0.25.1 mappings. The MP4 original date is the TagLib-compatible
`----:com.apple.iTunes:ORIGINALDATE` extension supported by Lofty; visibility in other
players depends on their support. New ID3 writes use the current adapter's ID3v2.4 output.
AAC remains read-only. Tag updates stage a copy and verify metadata, artwork and protected
identifiers before commit; unknown tag frames and audio content retain the existing adapter
preservation guarantees. Regression fixtures cover all eight writable formats, partial/full
dates, Unicode credits, unrelated edits and clears.

## Cleanup evidence and review

A confirmed MusicBrainz release supplies the edition date. Its release group's explicit
first-release date may supply the album original date; recording first-release-date remains
a distinct observation. A catalog year/month cannot erase a more precise matching authored
date. Edition alternatives and imported metadata remain unchecked proposals.

Composer proposals fill empty tags from explicit MusicBrainz work-composer relationships on
one performed work. They do not copy the artist, lyricist or a free-text credit. Medleys,
missing or malformed credit members, more than eight composers, or oversized text abstain.
The existing query already requests work/artist relationships, so this adds no provider
requests. Existing composer tags remain unchanged unless the operator edits them or selects
a sourced import replacement. See the [MusicBrainz API relationship includes](https://musicbrainz.org/doc/MusicBrainz_API)
and [composer relationship](https://musicbrainz.org/relationship/d59d99ea-23d4-4a80-b066-edca32ee158f).

A sourced JSON import with `propose: true` accepts `date`, `original_date` and
`composer` and turns them into the three writable fields. It can work when catalog identity
is unresolved, but the source label is an operator assertion, not independent verification.
All three fields participate in stale-evidence signatures, rejection/restoration and the
existing apply/undo journal. The catalog evidence policy version is 11, expiring older cached
proposals after the field contract changed. No file is changed by lookup or AI evaluation.
