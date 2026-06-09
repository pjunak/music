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
from __future__ import annotations

import math
import struct
from pathlib import Path

import pytest

from app.library import index as library_index

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


# --- minimal file builders ------------------------------------------------


def _wav(seconds: float = 0.2, sr: int = 8000) -> bytes:
    pcm = b"\x00\x00" * int(seconds * sr)
    return (
        b"RIFF" + struct.pack("<I", 36 + len(pcm)) + b"WAVE"
        + b"fmt " + struct.pack("<I", 16)
        + struct.pack("<HHIIHH", 1, 1, sr, sr * 2, 2, 16)
        + b"data" + struct.pack("<I", len(pcm)) + pcm
    )


def _float80(value: float) -> bytes:
    """IEEE-754 80-bit extended. AIFF stores its sample rate in this format
    and there's no stdlib helper (the `aifc` module was removed in 3.13)."""
    sign = 0
    if value < 0:
        sign, value = 1, -value
    if value == 0:
        return struct.pack(">HQ", 0, 0)
    mant, exp = math.frexp(value)  # value == mant * 2**exp, 0.5 <= mant < 1
    mant_int = int(round(mant * (1 << 64)))
    exp += 16382
    if mant_int == (1 << 64):  # rounding carried into the integer bit
        mant_int >>= 1
        exp += 1
    return struct.pack(">H", (sign << 15) | exp) + struct.pack(">Q", mant_int)


def _aiff(frames: int = 1600, sr: int = 8000) -> bytes:
    ssnd = struct.pack(">II", 0, 0) + b"\x00" * (frames * 2)
    comm = struct.pack(">hIh", 1, frames, 16) + _float80(sr)
    body = (
        b"AIFF"
        + b"COMM" + struct.pack(">I", len(comm)) + comm
        + b"SSND" + struct.pack(">I", len(ssnd)) + ssnd
    )
    return b"FORM" + struct.pack(">I", len(body)) + body


def _mp3() -> bytes:
    # 20 silent MPEG-1 Layer-3 frames (128 kbps / 44.1 kHz) — enough for
    # mutagen to sync to the stream and recognise it as MP3.
    header = bytes([0xFF, 0xFB, 0x90, 0x00])
    frame_len = 144 * 128000 // 44100
    return (header + b"\x00" * (frame_len - 4)) * 20


def _flac() -> bytes:
    # "fLaC" + one STREAMINFO block (last-block flag set), no audio frames.
    sr, ch, bps = 44100, 1, 16
    streaminfo = (
        struct.pack(">HH", 4096, 4096)
        + (0).to_bytes(3, "big") + (0).to_bytes(3, "big")  # min/max frame size
        + ((sr << 44) | ((ch - 1) << 41) | ((bps - 1) << 36)).to_bytes(8, "big")
        + b"\x00" * 16  # md5 signature (unknown)
    )
    return b"fLaC" + bytes([0x80]) + len(streaminfo).to_bytes(3, "big") + streaminfo


def _ogg_crc(data: bytes) -> int:
    """Ogg page CRC — poly 0x04C11DB7, no reflection, init 0."""
    crc = 0
    for byte in data:
        crc ^= byte << 24
        for _ in range(8):
            crc = ((crc << 1) ^ 0x04C11DB7) & 0xFFFFFFFF if crc & 0x80000000 else (crc << 1) & 0xFFFFFFFF
    return crc


def _ogg_page(serial: int, seq: int, packets: list[bytes], *, bos: bool = False, eos: bool = False) -> bytes:
    segs: list[int] = []
    for pkt in packets:
        n = len(pkt)
        while n >= 255:
            segs.append(255)
            n -= 255
        segs.append(n)
    header = (
        b"OggS\x00"
        + bytes([(0x02 if bos else 0) | (0x04 if eos else 0)])
        + struct.pack("<q", 0)        # granule position
        + struct.pack("<I", serial)
        + struct.pack("<I", seq)
        + b"\x00\x00\x00\x00"         # CRC placeholder, filled in below
        + bytes([len(segs)]) + bytes(segs)
    )
    page = header + b"".join(packets)
    return page[:22] + struct.pack("<I", _ogg_crc(page)) + page[26:]


def _ogg() -> bytes:
    # Three Vorbis header packets in Ogg pages. mutagen needs valid framing +
    # CRC and parses the identification/comment headers, but treats the setup
    # header body as opaque — so an 8-byte stub stands in for the codebooks.
    ident = (
        b"\x01vorbis" + struct.pack("<I", 0)
        + bytes([1]) + struct.pack("<I", 44100)
        + struct.pack("<iii", 0, 128000, 0)   # bitrate max / nominal / min
        + bytes([(8 << 4) | 8])               # blocksizes (256 / 256)
        + bytes([1])                          # framing bit
    )
    comment = b"\x03vorbis" + struct.pack("<I", 3) + b"min" + struct.pack("<I", 0) + bytes([1])
    setup = b"\x05vorbis" + b"\x00" * 8
    return (
        _ogg_page(1, 0, [ident], bos=True)
        + _ogg_page(1, 1, [comment, setup])
        + _ogg_page(1, 2, [b""], eos=True)
    )


_BUILDERS = {"wav": _wav, "aiff": _aiff, "mp3": _mp3, "flac": _flac, "ogg": _ogg}


# --- round-trip per format ------------------------------------------------


@pytest.mark.parametrize("ext", sorted(_BUILDERS))
def test_writable_tags_round_trip(tmp_path: Path, ext: str) -> None:
    """Every writable tag survives a write→read cycle, in every format —
    including `bpm` and `disc_no`, the two the four-site drift used to drop."""
    path = tmp_path / f"track.{ext}"
    path.write_bytes(_BUILDERS[ext]())

    library_index.write_tags(path, dict(SAMPLE))
    got = library_index._read_tags(path)

    for key, expected in SAMPLE.items():
        assert got.get(key) == expected, f"{ext}: {key} did not round-trip"


def test_clearing_a_writable_tag_round_trips(tmp_path: Path) -> None:
    """Writing an empty value clears the tag (the editor's "set to empty"
    path), and the read maps it back to the numeric/empty default."""
    path = tmp_path / "track.wav"
    path.write_bytes(_wav())
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
    path.write_bytes(_flac())
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
    path.write_bytes(_flac())

    meta = library_index.metadata_for(path, root=tmp_path)
    assert meta["album"] == "Vol.2"
    assert meta["track_no"] is None
    assert meta["bpm"] is None
