"""Synthetic audio containers used only by rewrite compatibility checks.

The fixtures contain silence or no audio frames, never user media.  Keeping the
builders here lets the Python oracle and its tag round-trip tests exercise the
same exact bytes without committing opaque binary files.
"""

from __future__ import annotations

import math
import struct
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

__all__ = ["MINIMAL_AUDIO_BUILDERS"]
