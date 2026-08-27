"""Synthetic audio containers used only by rewrite compatibility checks.

The fixtures contain silence or no audio frames, never user media.  Keeping the
builders here lets the Python oracle and its tag round-trip tests exercise the
same exact bytes without committing opaque binary files.
"""

from __future__ import annotations

import base64
import hashlib
import math
import struct
import zlib
from collections.abc import Callable


def _wav(seconds: float = 0.2, sample_rate: int = 8000) -> bytes:
    pcm = b"\x00\x00" * int(seconds * sample_rate)
    return (
        b"RIFF"
        + struct.pack("<I", 36 + len(pcm))
        + b"WAVE"
        + b"fmt "
        + struct.pack("<I", 16)
        + struct.pack("<HHIIHH", 1, 1, sample_rate, sample_rate * 2, 2, 16)
        + b"data"
        + struct.pack("<I", len(pcm))
        + pcm
    )


def _float80(value: float) -> bytes:
    """Encode the IEEE-754 80-bit extended value used by AIFF."""
    sign = 0
    if value < 0:
        sign, value = 1, -value
    if value == 0:
        return struct.pack(">HQ", 0, 0)
    mantissa, exponent = math.frexp(value)
    mantissa_integer = round(mantissa * (1 << 64))
    exponent += 16382
    if mantissa_integer == (1 << 64):
        mantissa_integer >>= 1
        exponent += 1
    return struct.pack(">H", (sign << 15) | exponent) + struct.pack(
        ">Q", mantissa_integer
    )


def _aiff(frames: int = 1600, sample_rate: int = 8000) -> bytes:
    sound_data = struct.pack(">II", 0, 0) + b"\x00" * (frames * 2)
    common = struct.pack(">hIh", 1, frames, 16) + _float80(sample_rate)
    body = (
        b"AIFF"
        + b"COMM"
        + struct.pack(">I", len(common))
        + common
        + b"SSND"
        + struct.pack(">I", len(sound_data))
        + sound_data
    )
    return b"FORM" + struct.pack(">I", len(body)) + body


def _mp3() -> bytes:
    # Twenty silent MPEG-1 Layer-3 frames are enough for Mutagen and Lofty to
    # synchronize to the stream and derive a duration.
    header = bytes([0xFF, 0xFB, 0x90, 0x00])
    frame_length = 144 * 128000 // 44100
    return (header + b"\x00" * (frame_length - 4)) * 20


def _flac() -> bytes:
    # fLaC plus one terminal STREAMINFO block; tags can be written without
    # requiring encoded audio frames.
    sample_rate, channels, bits_per_sample = 44100, 1, 16
    stream_info = (
        struct.pack(">HH", 4096, 4096)
        + (0).to_bytes(3, "big")
        + (0).to_bytes(3, "big")
        + (
            (sample_rate << 44)
            | ((channels - 1) << 41)
            | ((bits_per_sample - 1) << 36)
        ).to_bytes(8, "big")
        + b"\x00" * 16
    )
    return b"fLaC" + bytes([0x80]) + len(stream_info).to_bytes(3, "big") + stream_info


def _ogg_crc(data: bytes) -> int:
    crc = 0
    for byte in data:
        crc ^= byte << 24
        for _ in range(8):
            crc = (
                ((crc << 1) ^ 0x04C11DB7) & 0xFFFFFFFF
                if crc & 0x80000000
                else (crc << 1) & 0xFFFFFFFF
            )
    return crc


def _ogg_page(
    serial: int,
    sequence: int,
    packets: list[bytes],
    *,
    beginning: bool = False,
    end: bool = False,
) -> bytes:
    segments: list[int] = []
    for packet in packets:
        remaining = len(packet)
        while remaining >= 255:
            segments.append(255)
            remaining -= 255
        segments.append(remaining)
    header = (
        b"OggS\x00"
        + bytes([(0x02 if beginning else 0) | (0x04 if end else 0)])
        + struct.pack("<q", 0)
        + struct.pack("<I", serial)
        + struct.pack("<I", sequence)
        + b"\x00\x00\x00\x00"
        + bytes([len(segments)])
        + bytes(segments)
    )
    page = header + b"".join(packets)
    return page[:22] + struct.pack("<I", _ogg_crc(page)) + page[26:]


def _ogg() -> bytes:
    identification = (
        b"\x01vorbis"
        + struct.pack("<I", 0)
        + bytes([1])
        + struct.pack("<I", 44100)
        + struct.pack("<iii", 0, 128000, 0)
        + bytes([(8 << 4) | 8])
        + bytes([1])
    )
    comment = (
        b"\x03vorbis"
        + struct.pack("<I", 3)
        + b"min"
        + struct.pack("<I", 0)
        + bytes([1])
    )
    setup = b"\x05vorbis" + b"\x00" * 8
    return (
        _ogg_page(1, 0, [identification], beginning=True)
        + _ogg_page(1, 1, [comment, setup])
        + _ogg_page(1, 2, [b""], end=True)
    )


MINIMAL_AUDIO_BUILDERS: dict[str, Callable[[], bytes]] = {
    "wav": _wav,
    "aiff": _aiff,
    "mp3": _mp3,
    "flac": _flac,
    "ogg": _ogg,
}

# These four tiny silence containers cover formats whose framing is not
# practical to reproduce correctly with Python's standard library. They were
# generated with the checksum-pinned LGPL FFmpeg build recorded below. The
# encoded payloads are source data for deterministic builders, not user media.
FFMPEG_FIXTURE_PROVENANCE = {
    "version": "n9.0.1-8-g16dfae5c88-20260826",
    "archive": "ffmpeg-n9.0.1-8-g16dfae5c88-win64-lgpl-shared-9.0.zip",
    "archive_sha256": "120b26a83d10de8927297169c7f2417a6b6a544c51d5284e7183d864d4118cd2",
    "source": (
        "https://github.com/BtbN/FFmpeg-Builds/releases/download/"
        "autobuild-2026-08-26-13-06/"
    ),
}

_AAC_BASE64 = (
    "//FQQAF//AEYIAf/8VBAAX/8ARggB//xUEABf/wBGCAH//FQQAF//AEYIAf/8VBAAX/8ARggB//xUEABf/wBGCAH"
    "//FQQAF//AEYIAf/8VBAAX/8ARggB//xUEABf/wBGCAH//FQQAF//AEYIAf/8VBAAX/8ARggB//xUEABf/wBGCAH"
)

_M4A_BASE64 = (
    "AAAAHGZ0eXBNNEEgAAACAE00QSBpc29taXNvMgAAAtZtb292AAAAbG12aGQAAAAAAAAAAAAAAAAAAKxEAAArEQAB"
    "AAABAAAAAAAAAAAAAAAAAQAAAAAAAAAAAAAAAAAAAAEAAAAAAAAAAAAAAAAAAEAAAAAAAAAAAAAAAAAAAAAAAAAA"
    "AAAAAAAAAAAAAAACAAACJXRyYWsAAABcdGtoZAAAAAMAAAAAAAAAAAAAAAEAAAAAAAArEQAAAAAAAAAAAAAAAQEA"
    "AAAAAQAAAAAAAAAAAAAAAAAAAAEAAAAAAAAAAAAAAAAAAEAAAAAAAAAAAAAAAAAAACRlZHRzAAAAHGVsc3QAAAAA"
    "AAAAAQAAKxEAAAQAAAEAAAAAAZ1tZGlhAAAAIG1kaGQAAAAAAAAAAAAAAAAAAKxEAAAvEVXEAAAAAAAtaGRscgAA"
    "AAAAAAAAc291bgAAAAAAAAAAAAAAAFNvdW5kSGFuZGxlcgAAAAFIbWluZgAAABBzbWhkAAAAAAAAAAAAAAAkZGlu"
    "ZgAAABxkcmVmAAAAAAAAAAEAAAAMdXJsIAAAAAEAAAEMc3RibAAAAGpzdHNkAAAAAAAAAAEAAABabXA0YQAAAAAA"
    "AAABAAAAAAAAAAAAAQAQAAAAAKxEAAAAAAA2ZXNkcwAAAAADgICAJQABAASAgIAXQBUAAAAAAPoAAAAFfQWAgIAF"
    "EghW5QAGgICAAQIAAAAgc3R0cwAAAAAAAAACAAAACwAABAAAAAABAAADEQAAABxzdHNjAAAAAAAAAAEAAAABAAAA"
    "DAAAAAEAAAAUc3RzegAAAAAAAAAEAAAADAAAABRzdGNvAAAAAAAAAAEAAAMCAAAAGnNncGQBAAAAcm9sbAAAAAIA"
    "AAAB//8AAAAcc2JncAAAAAByb2xsAAAAAQAAAAwAAAABAAAAPXVkdGEAAAA1bWV0YQAAAAAAAAAhaGRscgAAAAAA"
    "AAAAbWRpcmFwcGwAAAAAAAAAAAAAAAAIaWxzdAAAAAhmcmVlAAAAOG1kYXQBGCAHARggBwEYIAcBGCAHARggBwEY"
    "IAcBGCAHARggBwEYIAcBGCAHARggBwEYIAc="
)

_OPUS_BASE64 = (
    "T2dnUwACAAAAAAAAAAAAAAAAAAAAAAIotXIBE09wdXNIZWFkAQE4AYC7AAAAAABPZ2dTAAAAAAAAAAAAAAAAAAAB"
    "AAAASZW+VAEuT3B1c1RhZ3MGAAAAZmZtcGVnAQAAABQAAABlbmNvZGVyPUxhdmMgbGlib3B1c09nZ1MABBgwAAAA"
    "AAAAAAAAAAIAAAB0mmfVDQMDAwMDAwMDAwMDAwP4//74//74//74//74//74//74//74//74//74//74//74//74"
    "//4="
)

_WMA_ZLIB_BASE64 = (
    "eNozUNtU2pd2XnDZTYZVDEnncroYGcCABYgZmRbeWd3jvvK8YN8ThgM8CsGpGQzYgQ0flNFgd/XexrmMUEMYFAqZ"
    "mUB0QrYWmC/DAxEHCTbwQDDDLwaGrcz74/VA9jyG2KMH1S946fLqXSDxZxBxNrDoRPY727cjiRZBVTvMy/zhG31e"
    "cMVfhob4GBftgLOH9/cnnhfs3gT025ZHCjDXygAxB8h/YF4i0LUua4C65RkYihkFGLjAohA5xmIgBDIdgi62yRpe"
    "EFy8hGHBSWaPbylQkxzRxBnBfhNnCGfIZMhjSGHIZyhnKGZQYPBlSAXyMhkSgWxHhlIwOx/IDmOwgIZHIqMZWkxs"
    "4sEe1rCwZWRsAgZQbBQziPOCoQ0S6BzFjJBgLmZs+O/AwMDe4DcK6AYYmeBx4DUaBwMUB8zwOKgYjYMBigMWeBws"
    "H42DAYoDVngcXB2NgwGKAzZ4HLDwjsbBgACGUTAKRgENAAA8pBFR"
)


def _encoded_fixture(value: str, expected_sha256: str) -> Callable[[], bytes]:
    def build() -> bytes:
        payload = base64.b64decode(value, validate=True)
        actual_sha256 = hashlib.sha256(payload).hexdigest()
        if actual_sha256 != expected_sha256:
            raise ValueError("encoded FFmpeg reference fixture failed its checksum")
        return payload

    return build


def _compressed_fixture(value: str, expected_sha256: str) -> Callable[[], bytes]:
    def build() -> bytes:
        payload = zlib.decompress(base64.b64decode(value, validate=True))
        actual_sha256 = hashlib.sha256(payload).hexdigest()
        if actual_sha256 != expected_sha256:
            raise ValueError("compressed FFmpeg reference fixture failed its checksum")
        return payload

    return build


FFMPEG_AUDIO_BUILDERS: dict[str, Callable[[], bytes]] = {
    "aac": _encoded_fixture(
        _AAC_BASE64, "1e17e860d5624eac32d16d87d63531191fc6770ec53a23bb2353c99d4495ef48"
    ),
    "m4a": _encoded_fixture(
        _M4A_BASE64, "c1b3ad9540d461b3d0a95a13b5cb4bc1be9a98590a17985d17d7c564e0760983"
    ),
    "opus": _encoded_fixture(
        _OPUS_BASE64, "5c8d228df5715f8d54564080d0bbcc7b59e584dc1040d4e9de8f2783282c265d"
    ),
    "wma": _compressed_fixture(
        _WMA_ZLIB_BASE64,
        "b15988ae1d011e5cabc8fac5a723f7bc37e680cfb17de7243f9b01e435381c4b",
    ),
}

REFERENCE_AUDIO_BUILDERS = MINIMAL_AUDIO_BUILDERS | FFMPEG_AUDIO_BUILDERS

__all__ = [
    "FFMPEG_AUDIO_BUILDERS",
    "FFMPEG_FIXTURE_PROVENANCE",
    "MINIMAL_AUDIO_BUILDERS",
    "REFERENCE_AUDIO_BUILDERS",
]
