"""Filesystem-driven library index.

The user's folder structure under MUSIC_DIR is the source of truth. We walk
the tree, read tags via mutagen when available, and materialise rows in the
`tracks` table. Path is the stable identity; mtime+size_bytes detects edits;
`added_at` is set the first time we see a file.

Invariants
----------
- `path` columns are always relative to MUSIC_DIR and use forward slashes.
  This keeps Linux server / Windows dev parity.
- We never move or rewrite files implicitly. Moves/renames are explicit
  user actions through the API; the index follows.
- An empty/missing MUSIC_DIR is a valid state — search returns []. Useful
  for fresh deploys before the first upload.
"""
from __future__ import annotations

import contextlib
import logging
import os
import threading
import time
from collections.abc import Iterable, Iterator
from dataclasses import dataclass
from pathlib import Path, PurePosixPath
from typing import Any

from mutagen import File as MutagenFile  # type: ignore[import-untyped]
from sqlalchemy import select
from sqlalchemy.orm import Session

from app.core.config import get_settings
from app.models.track import Track

logger = logging.getLogger(__name__)

# Serialises any index-write operation (full scan, incremental scan_paths
# from uploads). Without this, two concurrent scans race the DB:
# `scan_full` snapshots `existing` early, walks the disk, then deletes
# everything not seen — if a parallel `scan_paths` adds a row in between,
# the full scan's "delete unseen" step removes it again.
_scan_lock = threading.Lock()


# Audio extensions we'll consider when walking the tree. Lower-case match.
AUDIO_EXTENSIONS: frozenset[str] = frozenset(
    {".mp3", ".flac", ".ogg", ".opus", ".m4a", ".aac", ".wav", ".wma"}
)


@dataclass
class ScanSummary:
    added: int = 0
    updated: int = 0
    removed: int = 0
    unchanged: int = 0


# --- path helpers ---------------------------------------------------------


def music_root() -> Path:
    """The configured music directory. Created on demand so a fresh deploy
    boots cleanly even before the operator drops files in."""
    root = get_settings().music_dir.resolve()
    root.mkdir(parents=True, exist_ok=True)
    return root


def to_relative(absolute: Path, root: Path | None = None) -> str:
    """Convert an absolute path to the canonical (forward-slash, root-relative)
    form we store in the DB."""
    base = root if root is not None else music_root()
    rel = absolute.resolve().relative_to(base)
    return str(PurePosixPath(*rel.parts))


def to_absolute(rel: str, root: Path | None = None) -> Path:
    """Resolve a stored path back to an absolute one, with traversal guard."""
    base = root if root is not None else music_root()
    candidate = (base / rel).resolve()
    # Defence in depth: refuse paths that escape the music root, even if
    # the relative form looked harmless.
    candidate.relative_to(base)
    return candidate


def safe_join(parent_rel: str, name: str, root: Path | None = None) -> Path:
    """Build an absolute path inside MUSIC_DIR from a folder + filename,
    rejecting traversal attempts."""
    base = root if root is not None else music_root()
    parent_part = parent_rel.strip("/").replace("\\", "/")
    leaf = Path(name).name
    if not leaf or leaf in {".", ".."}:
        raise ValueError(f"invalid filename: {name!r}")
    target = (base / parent_part / leaf).resolve()
    target.relative_to(base)
    return target


def is_audio_file(path: Path) -> bool:
    return path.is_file() and path.suffix.lower() in AUDIO_EXTENSIONS


# --- tag extraction -------------------------------------------------------


def _coerce_int(value: Any) -> int | None:
    if value is None:
        return None
    if isinstance(value, int):
        return value
    text = str(value).strip()
    if not text:
        return None
    # Common patterns: "3", "3/12", "1990-01-01"
    head = text.split("/")[0].split("-")[0].strip()
    if not head:
        return None
    try:
        return int(head)
    except ValueError:
        return None


def _coerce_str(value: Any) -> str:
    if value is None:
        return ""
    if isinstance(value, list):
        return str(value[0]) if value else ""
    return str(value)


# Map our keys to (easy-interface keys, ID3 frame IDs). For each input key we
# try the easy keys first, then the ID3 frame IDs — so the same code reads
# Vorbis-tagged FLACs and ID3-tagged WAVs uniformly.
_TAG_LOOKUP: dict[str, tuple[tuple[str, ...], tuple[str, ...]]] = {
    "title": (("title",), ("TIT2",)),
    "artist": (("artist",), ("TPE1",)),
    "album_artist": (("albumartist", "album_artist"), ("TPE2",)),
    "album": (("album",), ("TALB",)),
    "track_no": (("tracknumber", "track"), ("TRCK",)),
    "disc_no": (("discnumber", "disc"), ("TPOS",)),
    "year": (("date", "year", "originaldate"), ("TDRC", "TYER", "TDRL")),
    "genre": (("genre",), ("TCON",)),
    "bpm": (("bpm",), ("TBPM",)),
}


def _id3_text(frame: Any) -> Any:
    """Extract a usable value from an ID3 Frame (e.g. TIT2.text → ['My Title'])."""
    text = getattr(frame, "text", None)
    if text is None:
        return None
    if isinstance(text, list):
        return [str(t) for t in text]
    return str(text)


def _read_tags(path: Path) -> dict[str, Any]:
    """Read what mutagen can. Anything mutagen can't parse falls back to
    empty/None and we use filename/folder for display.

    Handles both easy-string keying (FLAC/OGG/Opus/M4A/MP3-via-EasyID3) and
    raw ID3-frame keying (WAV/AIFF, where mutagen's easy=True doesn't fully
    apply). If the easy lookup misses, fall back to the frame ID."""
    try:
        meta = MutagenFile(str(path), easy=True)
    except Exception as e:
        logger.warning("failed to read tags from %s: %s", path, e)
        return {}
    if meta is None or meta.tags is None:
        return _info_only(meta)

    raw = meta.tags

    def lookup(easy_keys: tuple[str, ...], frame_ids: tuple[str, ...]) -> Any:
        for k in easy_keys:
            try:
                v = raw[k] if k in raw else raw.get(k) if hasattr(raw, "get") else None
            except (KeyError, ValueError):
                v = None
            if v:
                return v
        for fid in frame_ids:
            if fid in raw:
                return _id3_text(raw[fid])
        return None

    tags: dict[str, Any] = {}
    for our_key, (easy_keys, frame_ids) in _TAG_LOOKUP.items():
        value = lookup(easy_keys, frame_ids)
        if our_key in ("track_no", "disc_no", "year", "bpm"):
            tags[our_key] = _coerce_int(value)
        else:
            tags[our_key] = _coerce_str(value)

    info = getattr(meta, "info", None)
    if info is not None and getattr(info, "length", None):
        tags["length_s"] = float(info.length)

    return tags


def _info_only(meta: Any) -> dict[str, Any]:
    """Return just the duration if mutagen could read the file's `info`
    (audio properties) but not its tags (or there were none)."""
    if meta is None:
        return {}
    out: dict[str, Any] = {}
    info = getattr(meta, "info", None)
    if info is not None and getattr(info, "length", None):
        out["length_s"] = float(info.length)
    return out


def _filename_fallbacks(path: Path, parent_rel: str) -> dict[str, Any]:
    """When ID3 tags are empty, fall back to filename + folder so the UI
    isn't full of blanks."""
    return {
        "title": path.stem,
        "artist": "",
        "album_artist": "",
        "album": Path(parent_rel).name if parent_rel else "",
        "track_no": None,
        "disc_no": None,
        "year": None,
        "genre": "",
        "bpm": None,
        "length_s": 0.0,
    }


def metadata_for(path: Path, root: Path | None = None) -> dict[str, Any]:
    """Combined tag + fallback metadata for a single file.

    Used at insert time and by the metadata-edit endpoint to round-trip
    the same shape we store in the DB."""
    base = root if root is not None else music_root()
    rel = to_relative(path, base)
    parent_rel = str(PurePosixPath(rel).parent).replace(".", "")
    fallback = _filename_fallbacks(path, parent_rel)
    tags = _read_tags(path)
    merged: dict[str, Any] = {}
    for key, default in fallback.items():
        v = tags.get(key)
        if v is None or v == "":
            merged[key] = default
        else:
            merged[key] = v
    return merged


# --- scan -----------------------------------------------------------------


def _walk(root: Path) -> Iterator[Path]:
    """Yield every audio file under `root`, depth-first, deterministically."""
    if not root.is_dir():
        return
    for entry in sorted(root.iterdir(), key=lambda p: p.name.lower()):
        if entry.is_dir():
            yield from _walk(entry)
        elif is_audio_file(entry):
            yield entry


def _meta_dict(path: Path, root: Path) -> dict[str, Any]:
    """Build the field dict that both `_build_track` (insert) and
    `_refresh_track` (update) populate. Single source of truth for which
    columns are maintained from disk — adding a new tag-derived column
    means changing one place, not two."""
    stat = path.stat()
    meta = metadata_for(path, root)
    return {
        "title": meta["title"],
        "artist": meta["artist"],
        "album_artist": meta["album_artist"] or meta["artist"],
        "album": meta["album"],
        "track_no": meta["track_no"],
        "disc_no": meta["disc_no"],
        "year": meta["year"],
        "genre": meta["genre"],
        "length_s": meta["length_s"],
        "bpm": meta["bpm"],
        "size_bytes": stat.st_size,
        "mtime": int(stat.st_mtime),
    }


def _build_track(path: Path, root: Path) -> Track:
    return Track(path=to_relative(path, root), **_meta_dict(path, root))


def _refresh_track(track: Track, path: Path, root: Path) -> bool:
    """Re-read tags into an existing row. Returns True if anything changed."""
    changed = False
    for key, value in _meta_dict(path, root).items():
        if getattr(track, key) != value:
            setattr(track, key, value)
            changed = True
    return changed


def scan_full(db: Session) -> ScanSummary:
    """Walk MUSIC_DIR end-to-end. Inserts new rows, updates rows whose
    mtime/size changed on disk, and deletes rows whose paths no longer
    exist. Serialised with `_scan_lock` so a parallel upload-triggered
    `scan_paths` can't race the "delete unseen" tail of this function."""
    with _scan_lock:
        summary = ScanSummary()
        root = music_root()

        existing: dict[str, Track] = {
            t.path: t for t in db.scalars(select(Track)).all()
        }
        seen: set[str] = set()
        started = time.monotonic()

        for absolute in _walk(root):
            rel = to_relative(absolute, root)
            seen.add(rel)
            track = existing.get(rel)
            if track is None:
                db.add(_build_track(absolute, root))
                summary.added += 1
                continue
            stat = absolute.stat()
            if track.size_bytes != stat.st_size or track.mtime != int(stat.st_mtime):
                if _refresh_track(track, absolute, root):
                    summary.updated += 1
                else:
                    summary.unchanged += 1
            else:
                summary.unchanged += 1

        # Anything in the DB not seen on disk got removed.
        for rel, track in existing.items():
            if rel not in seen:
                db.delete(track)
                summary.removed += 1

        db.commit()
        elapsed = time.monotonic() - started
        logger.info(
            "scan: +%d ~%d -%d (%d unchanged) in %.2fs",
            summary.added,
            summary.updated,
            summary.removed,
            summary.unchanged,
            elapsed,
        )
        return summary


def scan_paths(db: Session, paths: Iterable[Path]) -> list[Track]:
    """Index/refresh a specific set of paths (e.g. just-uploaded files).

    Returns the resulting Track rows in the same order as input. Skips
    non-audio files silently. Caller commits.

    Shares `_scan_lock` with `scan_full` — a parallel full scan that's
    snapshotting `existing` rows mustn't race a fresh insert here, or
    the new row will get pruned as "unseen on disk" (it was added after
    the snapshot) when the full scan finishes. The lock is short-held
    in this function (per-file flushes are fast)."""
    with _scan_lock:
        root = music_root()
        out: list[Track] = []
        for absolute in paths:
            if not is_audio_file(absolute):
                continue
            rel = to_relative(absolute, root)
            existing = db.scalar(select(Track).where(Track.path == rel))
            if existing is None:
                track = _build_track(absolute, root)
                db.add(track)
                db.flush()
                out.append(track)
            else:
                _refresh_track(existing, absolute, root)
                out.append(existing)
        db.commit()
        return out


def remove_path(db: Session, rel: str) -> bool:
    """Drop the row for `rel` (e.g. after the file was deleted on disk).
    Returns True if a row existed."""
    row = db.scalar(select(Track).where(Track.path == rel))
    if row is None:
        return False
    db.delete(row)
    db.commit()
    return True


def update_path(db: Session, old_rel: str, new_rel: str) -> Track | None:
    """Rename/move bookkeeping after a file has already moved on disk."""
    row = db.scalar(select(Track).where(Track.path == old_rel))
    if row is None:
        return None
    row.path = new_rel
    # Re-read tags too, since album-fallback (= parent folder) may have changed.
    abs_path = to_absolute(new_rel)
    if abs_path.is_file():
        _refresh_track(row, abs_path, music_root())
    db.commit()
    return row


# --- tag writeback (metadata editor) -------------------------------------


_WRITABLE_TAGS = ("title", "artist", "album_artist", "album", "track_no", "year", "genre")

# WAV / AIFF embed ID3 inside a RIFF chunk; mutagen exposes them via the
# format-specific class (WAVE / AIFF) but the tag dict only accepts ID3
# Frame instances, not the easy-string interface. MP3 has EasyID3 that
# wraps both. Everything else (FLAC / OGG / Opus / M4A) supports the
# easy-string dict directly via mutagen.File(..., easy=True).
_ID3_FRAME_FORMATS = frozenset({".wav", ".aiff", ".aif"})


def write_tags(path: Path, fields: dict[str, Any]) -> None:
    """Write the given fields back to the file's tags. Only `_WRITABLE_TAGS`
    are honoured; unknown keys are ignored. ints get coerced to strings."""
    suffix = path.suffix.lower()

    if suffix == ".mp3":
        from mutagen.easyid3 import EasyID3
        from mutagen.id3 import ID3, ID3NoHeaderError

        try:
            tags = EasyID3(str(path))
        except ID3NoHeaderError:
            ID3().save(str(path))
            tags = EasyID3(str(path))
        _apply_easy(tags, fields)
        tags.save()
        return

    if suffix in _ID3_FRAME_FORMATS:
        # WAV / AIFF: write ID3 frames directly. Mutagen has WAVE and AIFF
        # classes that store an ID3 dict at .tags.
        from mutagen.id3 import TALB, TCON, TDRC, TIT2, TPE1, TPE2, TRCK
        from mutagen.wave import WAVE

        wav_meta: Any = WAVE(str(path)) if suffix == ".wav" else None
        if wav_meta is None:
            from mutagen.aiff import AIFF

            wav_meta = AIFF(str(path))
        if wav_meta.tags is None:
            wav_meta.add_tags()

        frames = {
            "title": TIT2,
            "artist": TPE1,
            "album_artist": TPE2,
            "album": TALB,
            "track_no": TRCK,
            "year": TDRC,
            "genre": TCON,
        }
        for key, FrameClass in frames.items():
            if key not in fields or key not in _WRITABLE_TAGS:
                continue
            value = fields[key]
            frame_id = FrameClass.__name__
            if value is None or value == "":
                wav_meta.tags.delall(frame_id)
            else:
                wav_meta.tags.add(FrameClass(encoding=3, text=str(value)))
        wav_meta.save()
        return

    # FLAC / OGG / Opus / M4A: easy string-list interface on the tag dict.
    meta = MutagenFile(str(path), easy=True)
    if meta is None:
        raise ValueError(f"unsupported audio format for {path.name}")
    if meta.tags is None:
        meta.add_tags()
    _apply_easy(meta.tags, fields)
    meta.save()


def _apply_easy(tag_dict: Any, fields: dict[str, Any]) -> None:
    mapping = {
        "title": "title",
        "artist": "artist",
        "album_artist": "albumartist",
        "album": "album",
        "track_no": "tracknumber",
        "year": "date",
        "genre": "genre",
    }
    for key, mutagen_key in mapping.items():
        if key not in fields or key not in _WRITABLE_TAGS:
            continue
        value = fields[key]
        if value is None or value == "":
            with contextlib.suppress(KeyError):
                del tag_dict[mutagen_key]
        else:
            tag_dict[mutagen_key] = [str(value)]


# --- folder navigation ---------------------------------------------------


@dataclass
class FolderEntry:
    """A directory immediately under some folder. Track count is recursive
    (includes subfolders) so the tree gives a useful at-a-glance density."""

    name: str
    path: str  # canonical relative path, forward slashes
    track_count: int


def list_folder(db: Session, rel_path: str = "") -> tuple[list[FolderEntry], list[Track]]:
    """Direct contents of `rel_path`: subfolders and tracks immediately
    under it (not recursive). Track counts on subfolders ARE recursive so
    the tree gives a useful density signal."""
    from sqlalchemy import func

    root = music_root()
    rel_clean = rel_path.strip("/").replace("\\", "/")
    abs_dir = (root / rel_clean).resolve() if rel_clean else root
    abs_dir.relative_to(root)  # traversal guard

    if not abs_dir.is_dir():
        return ([], [])

    folders: list[FolderEntry] = []
    for child in sorted(abs_dir.iterdir(), key=lambda p: p.name.lower()):
        if not child.is_dir():
            continue
        sub_rel = to_relative(child, root)
        count = (
            db.scalar(
                select(func.count(Track.id)).where(Track.path.like(f"{sub_rel}/%"))
            )
            or 0
        )
        folders.append(FolderEntry(name=child.name, path=sub_rel, track_count=count))

    # Direct children only. "Skyrim/foo.mp3" is a child of Skyrim;
    # "Skyrim/sub/bar.mp3" isn't. Encoded as two LIKE clauses so SQLite
    # filters server-side instead of dragging every row into Python on
    # every folder browse.
    if rel_clean:
        prefix = f"{rel_clean}/"
        tracks_query = (
            select(Track)
            .where(Track.path.like(f"{prefix}%"))
            .where(~Track.path.like(f"{prefix}%/%"))
            .order_by(Track.path)
        )
    else:
        # Root listing: top-level files have no slash in their relative path.
        tracks_query = (
            select(Track)
            .where(~Track.path.like("%/%"))
            .order_by(Track.path)
        )
    tracks = list(db.scalars(tracks_query).all())
    return (folders, tracks)


def ensure_folder(rel_path: str, root: Path | None = None) -> Path:
    """mkdir -p inside the given root (defaults to MUSIC_DIR). Returns
    the absolute path of the resulting directory."""
    base = root if root is not None else music_root()
    rel_clean = rel_path.strip("/").replace("\\", "/")
    if not rel_clean:
        return base
    target = (base / rel_clean).resolve()
    target.relative_to(base)
    target.mkdir(parents=True, exist_ok=True)
    return target


def delete_folder(
    db: Session | None,
    rel_path: str,
    *,
    recursive: bool,
    root: Path | None = None,
) -> int:
    """Delete a folder and (when `recursive`) everything in it.

    For the music root, also deletes any `tracks` rows whose path lives
    inside this folder; returns how many rows were dropped (callers can
    surface that in the UI). Pass `db=None` for non-indexed roots (SFX).
    """
    import shutil as _shutil

    base = root if root is not None else music_root()
    rel_clean = rel_path.strip("/").replace("\\", "/")
    if not rel_clean:
        raise ValueError("refusing to delete the root directory")
    target = (base / rel_clean).resolve()
    target.relative_to(base)
    if not target.exists():
        raise FileNotFoundError(f"folder not found: {rel_clean}")
    if not target.is_dir():
        raise ValueError(f"not a folder: {rel_clean}")

    if not recursive and any(target.iterdir()):
        raise ValueError("folder is not empty (pass recursive=true to force)")

    removed_rows = 0
    if db is not None:
        prefix = f"{rel_clean}/"
        rows = db.scalars(
            select(Track).where(
                (Track.path == rel_clean) | (Track.path.like(f"{prefix}%"))
            )
        ).all()
        for row in rows:
            db.delete(row)
            removed_rows += 1

    if recursive:
        _shutil.rmtree(target)
    else:
        target.rmdir()

    if db is not None and removed_rows > 0:
        db.commit()
    return removed_rows


def rename_folder(
    db: Session | None,
    src_rel: str,
    dst_rel: str,
    root: Path | None = None,
) -> int:
    """Rename / move a folder. For the music root, also rewrites every
    `tracks.path` whose value lives under the renamed folder; returns the
    number of rewritten rows. Pass `db=None` for non-indexed roots (SFX)."""
    import shutil as _shutil

    base = root if root is not None else music_root()
    src_clean = src_rel.strip("/").replace("\\", "/")
    dst_clean = dst_rel.strip("/").replace("\\", "/")
    if not src_clean or not dst_clean:
        raise ValueError("source and destination must be non-empty paths")

    src_abs = (base / src_clean).resolve()
    dst_abs = (base / dst_clean).resolve()
    src_abs.relative_to(base)
    dst_abs.relative_to(base)
    if not src_abs.is_dir():
        raise FileNotFoundError(f"source folder not found: {src_clean}")
    if dst_abs.exists():
        raise FileExistsError(f"destination already exists: {dst_clean}")

    dst_abs.parent.mkdir(parents=True, exist_ok=True)
    _shutil.move(str(src_abs), str(dst_abs))

    rewritten = 0
    if db is not None:
        prefix = f"{src_clean}/"
        rows = db.scalars(
            select(Track).where(
                (Track.path == src_clean) | (Track.path.like(f"{prefix}%"))
            )
        ).all()
        for row in rows:
            row.path = dst_clean + row.path[len(src_clean):]
            rewritten += 1
        if rewritten > 0:
            db.commit()
    return rewritten


def cover_art_for(track: Track) -> tuple[bytes, str] | None:
    """Return (image_bytes, mime_type) for the track's cover art if any.

    Order of preference:
    1. Embedded artwork (ID3 APIC frame, FLAC pictures, MP4 covr).
    2. `cover.jpg` / `cover.png` / `folder.jpg` / `folder.png` next to the file.
    """
    abs_path = to_absolute(track.path)
    if not abs_path.is_file():
        return None

    try:
        meta = MutagenFile(str(abs_path))
    except Exception:
        meta = None

    if meta is not None:
        # ID3 (mp3): frames keyed APIC:*
        for key, frame in (meta.tags or {}).items():
            if key.startswith("APIC") and getattr(frame, "data", None):
                mime = getattr(frame, "mime", "image/jpeg")
                return (bytes(frame.data), mime)
        # FLAC: meta.pictures
        pics = getattr(meta, "pictures", None)
        if pics:
            pic = pics[0]
            return (bytes(pic.data), getattr(pic, "mime", "image/jpeg"))
        # MP4: 'covr' atom
        covr = (meta.tags or {}).get("covr")
        if covr:
            data = covr[0]
            return (bytes(data), "image/jpeg")

    # Folder fallback.
    parent = abs_path.parent
    for name in ("cover.jpg", "cover.jpeg", "cover.png", "folder.jpg", "folder.png"):
        candidate = parent / name
        if candidate.is_file():
            mime = "image/png" if candidate.suffix.lower() == ".png" else "image/jpeg"
            return (candidate.read_bytes(), mime)

    return None


# --- minor helpers --------------------------------------------------------


def relative_audio_files_in(absolute_dir: Path, root: Path | None = None) -> list[Path]:
    """All audio paths under absolute_dir as absolute Paths. Used after an
    upload to scan only what just landed."""
    base = root if root is not None else music_root()
    if not absolute_dir.is_dir():
        return []
    # Traversal guard
    absolute_dir = absolute_dir.resolve()
    absolute_dir.relative_to(base)
    return list(_walk(absolute_dir))


# Re-exports so `os` module access from callers stays minimal.
__all__ = [
    "AUDIO_EXTENSIONS",
    "FolderEntry",
    "ScanSummary",
    "cover_art_for",
    "delete_folder",
    "ensure_folder",
    "is_audio_file",
    "list_folder",
    "metadata_for",
    "music_root",
    "relative_audio_files_in",
    "remove_path",
    "rename_folder",
    "safe_join",
    "scan_full",
    "scan_paths",
    "to_absolute",
    "to_relative",
    "update_path",
    "write_tags",
]


# Silence unused-import warning for `os` in this top-level file (used by
# tests via patch).
_ = os
