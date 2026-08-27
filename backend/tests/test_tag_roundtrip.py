"""Tag write/read round-trips per audio format + registry-consistency guards.

The library's tag<->format mapping lives in one declarative `TAG_REGISTRY`
(`app/library/index.py`); the read lookup, the WAV/AIFF frame-class dict, the
easy-write mapping, and `WRITABLE_TAGS` are all *derived* from it. These tests
pin that single source down two ways:

  * round-trip — build a minimal real file per format, write every writable
    tag through `write_tags`, read it back through `_read_tags`, assert it
    survives. Covers all three `write_tags` code paths: MP3/EasyID3, WAV/AIFF
    ID3 frames, and the FLAC/OGG easy-string dict.
  * consistency — assert the derived views actually agree with the registry,
    so a future hand-edit to one view (the four-site drift this refactor
    removes) is caught instead of silently dropping a tag.

Files are built in pure Python — no binary fixtures, no encoder. Silence is
enough for mutagen to parse tags; FLAC/OGG carry no audio frames at all
(mutagen reads/writes their comment blocks without decoding a stream).
"""
from pathlib import Path

import pytest

from app.library import index as library_index
from app.reference_media import MINIMAL_AUDIO_BUILDERS

# One representative value per writable tag — a mix of str and int so the
# coerce path is exercised both ways.
SAMPLE: dict[str, object] = {
    "title": "Round Trip",
    "artist": "The Artist",
    "album_artist": "Album Artist",
    "album": "An Album",
    "track_no": 7,
    "disc_no": 2,
    "year": 1991,
    "genre": "Ambient",
    "bpm": 123,
}


# --- round-trip per format ------------------------------------------------


@pytest.mark.parametrize("ext", sorted(MINIMAL_AUDIO_BUILDERS))
def test_writable_tags_round_trip(tmp_path: Path, ext: str) -> None:
    """Every writable tag survives a write→read cycle, in every format —
    including `bpm` and `disc_no`, the two the four-site drift used to drop."""
    path = tmp_path / f"track.{ext}"
    path.write_bytes(MINIMAL_AUDIO_BUILDERS[ext]())

    library_index.write_tags(path, dict(SAMPLE))
    got = library_index._read_tags(path)

    for key, expected in SAMPLE.items():
        assert got.get(key) == expected, f"{ext}: {key} did not round-trip"


def test_clearing_a_writable_tag_round_trips(tmp_path: Path) -> None:
    """Writing an empty value clears the tag (the editor's "set to empty"
    path), and the read maps it back to the numeric/empty default."""
    path = tmp_path / "track.wav"
    path.write_bytes(MINIMAL_AUDIO_BUILDERS["wav"]())
    library_index.write_tags(path, dict(SAMPLE))

    library_index.write_tags(path, {"genre": "", "bpm": None})
    got = library_index._read_tags(path)
    assert got["genre"] == ""
    assert got["bpm"] is None
    assert got["title"] == SAMPLE["title"]  # untouched fields stay


# --- registry consistency (the drift guard) -------------------------------


def test_derived_views_agree_with_registry() -> None:
    """The four derived structures must agree with `TAG_REGISTRY` — this is
    the invariant the refactor exists to enforce. A hand-edit to any one
    view that diverges from the table fails here."""
    registry = library_index.TAG_REGISTRY
    writable = set(library_index.WRITABLE_TAGS)

    assert set(SAMPLE) == writable, "SAMPLE must cover exactly the writable tags"
    assert set(library_index._WAV_FRAME_CLASSES) == writable
    assert set(library_index._EASY_WRITE_KEYS) == writable

    for key, spec in registry.items():
        assert library_index._TAG_LOOKUP[key] == (spec.read_easy_keys, spec.id3_frame_ids)
        if not spec.writable:
            continue
        assert library_index._WAV_FRAME_CLASSES[key] is spec.id3_frame_class
        assert library_index._EASY_WRITE_KEYS[key] == spec.write_easy_key
        # What you write you must be able to read back: the one write spelling
        # has to be among the accepted read spellings, or a tag would persist
        # to disk yet never load.
        assert spec.write_easy_key in spec.read_easy_keys
        assert spec.id3_frame_class.__name__ in spec.id3_frame_ids


def test_numeric_tags_coerce_to_int_on_read(tmp_path: Path) -> None:
    """track_no/disc_no/year/bpm come back as ints; the rest as strings —
    and a list-wrapped tag value (how mutagen hands them over) is unwrapped,
    not stringified into None."""
    int_keys = {k for k, s in library_index.TAG_REGISTRY.items() if s.coerce is library_index._coerce_int}
    assert int_keys == {"track_no", "disc_no", "year", "bpm"}

    path = tmp_path / "track.flac"
    path.write_bytes(MINIMAL_AUDIO_BUILDERS["flac"]())
    library_index.write_tags(path, dict(SAMPLE))
    got = library_index._read_tags(path)
    for key in int_keys:
        assert isinstance(got[key], int)


# --- album fallback (dotted folder, non-WAV) ------------------------------


def test_album_falls_back_to_dotted_parent_folder(tmp_path: Path) -> None:
    """An untagged file takes its album from the parent folder name verbatim
    — dots intact — across formats, while numeric tags stay None (not 0)."""
    folder = tmp_path / "Vol.2"
    folder.mkdir()
    path = folder / "song.flac"
    path.write_bytes(MINIMAL_AUDIO_BUILDERS["flac"]())

    meta = library_index.metadata_for(path, root=tmp_path)
    assert meta["album"] == "Vol.2"
    assert meta["track_no"] is None
    assert meta["bpm"] is None
