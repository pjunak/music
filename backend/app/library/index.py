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
import shutil
import threading
import time
from collections.abc import Callable, Iterable, Iterator
from dataclasses import dataclass
from pathlib import Path, PurePosixPath
from typing import Any

from mutagen import File as MutagenFile  # type: ignore[import-untyped]
from mutagen.id3 import (  # type: ignore[import-untyped]
    TALB,
    TBPM,
    TCON,
    TDRC,
    TIT2,
    TPE1,
    TPE2,
    TPOS,
    TRCK,
    Frame,
)
from sqlalchemy import func, select
from sqlalchemy.orm import Session

from app.core.config import get_settings
from app.models.track import Track

logger = logging.getLogger(__name__)

# Serialises any index-write operation (full scan, incremental scan_paths
# from uploads). Without this, two concurrent scans race the DB:
# `scan_full` snapshots `existing` early, walks the disk, then deletes
# everything not seen - if a parallel `scan_paths` adds a row in between,
# the full scan's "delete unseen" step removes it again.
_scan_lock = threading.Lock()

# Wall-clock timestamp of the last scan_full completion. Read by the
# diagnostics endpoint so the operator can see how stale the index is.
_last_scan_at: float | None = None

# Fired after any index write commits (scans, deletes, renames). Lets a
# holder of caches keyed on track rows — the advancer's length cache —
# drop them when a row may have changed underneath (e.g. a file replaced
# in place gets a new duration under the same track id). Registered by
# `Advancer.start()`; may be invoked from a worker thread.
on_index_changed: Callable[[], None] | None = None


def _notify_index_changed() -> None:
    hook = on_index_changed
    if hook is not None:
        hook()


def last_scan_at() -> float | None:
    """Returns the unix timestamp of the most recent successful full scan,
    or None if no full scan has run since startup."""
    return _last_scan_at


# Audio extensions we'll consider when walking the tree. Lower-case match.
AUDIO_EXTENSIONS: frozenset[str] = frozenset(
    {".mp3", ".flac", ".ogg", ".opus", ".m4a", ".aac", ".wav", ".wma"}
)

# Streamed upload chunk size. 1 MiB balances memory use against syscall count.
UPLOAD_CHUNK = 1 << 20


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


def resolve_conflict(target: Path, conflict: str) -> Path | None:
    """Decide the final write path for an upload given an existing-file policy.

    Returns the path to write to, or ``None`` when the policy says to skip
    the file. ``conflict`` is one of:

    - ``"overwrite"`` — write to ``target`` even if it exists (replace it).
    - ``"skip"``      — leave the existing file; return ``None``.
    - ``"rename"``    — keep both: ``foo.mp3`` -> ``foo-1.mp3``, ``foo-2.mp3``…
                        (the default, and what happens when nothing collides).
    """
    if not target.exists():
        return target
    if conflict == "overwrite":
        return target
    if conflict == "skip":
        return None
    stem, suffix = target.stem, target.suffix
    n = 1
    while True:
        candidate = target.with_name(f"{stem}-{n}{suffix}")
        if not candidate.exists():
            return candidate
        n += 1


LIKE_ESCAPE_CHAR = "\\"


def like_escape(value: str) -> str:
    r"""Escape SQL ``LIKE`` metacharacters so `value` matches literally.

    SQLite's ``LIKE`` reads ``_`` as any single char and ``%`` as any run, so
    a folder name interpolated raw into a prefix pattern over-matches: scoping
    to ``Skyrim_OST`` would also pull in ``SkyrimXOST``. Escape the interpolated
    span and pass ``escape=LIKE_ESCAPE_CHAR`` to ``.like()``; keep real
    wildcards (the trailing ``%``) outside the escaped span. Backslash is
    escaped first so the escapes we add aren't themselves re-escaped."""
    return value.replace("\\", "\\\\").replace("%", "\\%").replace("_", "\\_")


def is_audio_file(path: Path) -> bool:
    return path.is_file() and path.suffix.lower() in AUDIO_EXTENSIONS


# --- tag extraction -------------------------------------------------------


def _coerce_int(value: Any) -> int | None:
    if value is None:
        return None
    if isinstance(value, int):
        return value
    # mutagen hands tags back as single-element lists (easy interface) or
    # ID3 frame text lists — unwrap before parsing, same as _coerce_str.
    if isinstance(value, list):
        value = value[0] if value else None
        if value is None:
            return None
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


@dataclass(frozen=True)
class TagSpec:
    """How one logical tag maps across formats — the single source of truth.

    Reading is *many-spellings-in*: try `read_easy_keys` on the easy-string
    interface (FLAC/OGG/Opus/M4A and MP3-via-EasyID3), then fall back to the
    raw `id3_frame_ids` for WAV/AIFF, whose tag dict speaks ID3 frames, not
    easy keys. Writing is *one-spelling-out*: emit `write_easy_key` on the
    easy interface and an `id3_frame_class` instance on the WAV/AIFF frame
    dict (the frame *class*, not just its id, because writing constructs it).
    `coerce` normalises the read value (int for numeric tags, str otherwise);
    `writable` gates whether the field round-trips back to disk at all.
    """

    read_easy_keys: tuple[str, ...]
    id3_frame_ids: tuple[str, ...]
    id3_frame_class: type[Frame]
    write_easy_key: str
    writable: bool
    coerce: Callable[[Any], Any]


# The one table. Every read/write mapping below is DERIVED from this — never
# hand-maintain the derived views, edit a row here and they all follow.
#                read_easy_keys                       id3_frame_ids              class  write_easy_key  writable  coerce
TAG_REGISTRY: dict[str, TagSpec] = {
    "title":        TagSpec(("title",),                    ("TIT2",),                 TIT2, "title",       True, _coerce_str),
    "artist":       TagSpec(("artist",),                   ("TPE1",),                 TPE1, "artist",      True, _coerce_str),
    "album_artist": TagSpec(("albumartist", "album_artist"), ("TPE2",),               TPE2, "albumartist", True, _coerce_str),
    "album":        TagSpec(("album",),                    ("TALB",),                 TALB, "album",       True, _coerce_str),
    "track_no":     TagSpec(("tracknumber", "track"),      ("TRCK",),                 TRCK, "tracknumber", True, _coerce_int),
    "disc_no":      TagSpec(("discnumber", "disc"),        ("TPOS",),                 TPOS, "discnumber",  True, _coerce_int),
    "year":         TagSpec(("date", "year", "originaldate"), ("TDRC", "TYER", "TDRL"), TDRC, "date",      True, _coerce_int),
    "genre":        TagSpec(("genre",),                    ("TCON",),                 TCON, "genre",       True, _coerce_str),
    "bpm":          TagSpec(("bpm",),                      ("TBPM",),                 TBPM, "bpm",          True, _coerce_int),
}

# --- derived views (do not edit — change TAG_REGISTRY) --------------------
# Read lookup: our key -> (easy keys, ID3 frame ids). Used by `_read_tags`.
_TAG_LOOKUP: dict[str, tuple[tuple[str, ...], tuple[str, ...]]] = {
    key: (spec.read_easy_keys, spec.id3_frame_ids) for key, spec in TAG_REGISTRY.items()
}
# WAV/AIFF write: our key -> ID3 Frame class to construct.
_WAV_FRAME_CLASSES: dict[str, type[Frame]] = {
    key: spec.id3_frame_class for key, spec in TAG_REGISTRY.items() if spec.writable
}
# Easy-interface write (FLAC/OGG/Opus/M4A/MP3): our key -> canonical easy key.
_EASY_WRITE_KEYS: dict[str, str] = {
    key: spec.write_easy_key for key, spec in TAG_REGISTRY.items() if spec.writable
}
# Keys whose edits round-trip to the file's tags. Public: the API layer
# derives its tag-backed field set from this so the two can't disagree.
WRITABLE_TAGS: tuple[str, ...] = tuple(
    key for key, spec in TAG_REGISTRY.items() if spec.writable
)


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
        tags[our_key] = TAG_REGISTRY[our_key].coerce(value)

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
    parent = PurePosixPath(rel).parent
    parent_rel = "" if str(parent) == "." else str(parent)
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
        _notify_index_changed()
        elapsed = time.monotonic() - started
        global _last_scan_at
        _last_scan_at = time.time()
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
    non-audio files silently. Commits before returning.

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
        _notify_index_changed()
        return out


def remove_path(db: Session, rel: str) -> bool:
    """Drop the row for `rel` (e.g. after the file was deleted on disk).
    Returns True if a row existed."""
    row = db.scalar(select(Track).where(Track.path == rel))
    if row is None:
        return False
    db.delete(row)
    db.commit()
    _notify_index_changed()
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
    _notify_index_changed()
    return row


def read_file_tags(path: Path) -> dict[str, Any]:
    """The file's actual tag state: coerced TAG_REGISTRY values for tags
    present in the file, with absent string tags as ""/missing and absent
    numeric tags as None/missing. Unlike `metadata_for`, NO filename/folder
    fallbacks are applied — callers that need to know what's really written
    in the file (e.g. cleanup journaling for faithful reverts) use this."""
    return _read_tags(path)


# --- tag writeback (metadata editor) -------------------------------------


# WAV / AIFF embed ID3 inside a RIFF chunk; mutagen exposes them via the
# format-specific class (WAVE / AIFF) but the tag dict only accepts ID3
# Frame instances, not the easy-string interface. MP3 has EasyID3 that
# wraps both. Everything else (FLAC / OGG / Opus / M4A) supports the
# easy-string dict directly via mutagen.File(..., easy=True).
_ID3_FRAME_FORMATS = frozenset({".wav", ".aiff", ".aif"})


def write_tags(path: Path, fields: dict[str, Any]) -> None:
    """Write the given fields back to the file's tags. Only `WRITABLE_TAGS`
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
        from mutagen.wave import WAVE

        wav_meta: Any = WAVE(str(path)) if suffix == ".wav" else None
        if wav_meta is None:
            from mutagen.aiff import AIFF

            wav_meta = AIFF(str(path))
        if wav_meta.tags is None:
            wav_meta.add_tags()
        wav_tags: Any = wav_meta.tags

        for key, frame_class in _WAV_FRAME_CLASSES.items():
            if key not in fields:
                continue
            value = fields[key]
            if value is None or value == "":
                wav_tags.delall(frame_class.__name__)
            else:
                wav_tags.add(frame_class(encoding=3, text=str(value)))
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
    for key, mutagen_key in _EASY_WRITE_KEYS.items():
        if key not in fields:
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
    (includes subfolders) so the tree gives a useful at-a-glance density.
    `has_children` lets the tree UI hide the expand-toggle on leaf folders
    so empty subdir-less folders don't pretend they can be expanded."""

    name: str
    path: str  # canonical relative path, forward slashes
    track_count: int
    has_children: bool


def track_ids_under(db: Session, rel_path: str) -> list[int]:
    """All track ids at or under `rel_path` (recursive), in library order
    (`path` ascending — the same ordering the folder browser uses). An empty
    `rel_path` returns the whole library. Used to load a folder/album into the
    ambient queue."""
    rel_clean = rel_path.strip("/").replace("\\", "/")
    stmt = select(Track.id)
    if rel_clean:
        prefix = like_escape(f"{rel_clean}/")
        stmt = stmt.where(
            (Track.path == rel_clean)
            | (Track.path.like(f"{prefix}%", escape=LIKE_ESCAPE_CHAR))
        )
    stmt = stmt.order_by(Track.path)
    return [int(r) for r in db.scalars(stmt).all()]


def next_track_id_after(db: Session, path: str, *, wrap: bool = True) -> int | None:
    """The id of the track that immediately follows `path` in library order
    (`path` ascending). Drives "follow / continue" playback. With `wrap`
    (default), the library's first track follows its last so follow never runs
    out; with ``wrap=False`` the end of the library returns None. Returns None
    when the library is empty."""
    nxt = db.scalar(
        select(Track.id).where(Track.path > path).order_by(Track.path).limit(1)
    )
    if nxt is not None:
        return int(nxt)
    if not wrap:
        return None
    first = db.scalar(select(Track.id).order_by(Track.path).limit(1))
    return int(first) if first is not None else None


def list_folder(db: Session, rel_path: str = "") -> tuple[list[FolderEntry], list[Track]]:
    """Direct contents of `rel_path`: subfolders and tracks immediately
    under it (not recursive). Track counts on subfolders ARE recursive so
    the tree gives a useful density signal."""
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
                select(func.count(Track.id)).where(
                    Track.path.like(f"{like_escape(sub_rel)}/%", escape=LIKE_ESCAPE_CHAR)
                )
            )
            or 0
        )
        has_children = any(grandchild.is_dir() for grandchild in child.iterdir())
        folders.append(
            FolderEntry(
                name=child.name,
                path=sub_rel,
                track_count=count,
                has_children=has_children,
            )
        )

    # Direct children only. "Skyrim/foo.mp3" is a child of Skyrim;
    # "Skyrim/sub/bar.mp3" isn't. Encoded as two LIKE clauses so SQLite
    # filters server-side instead of dragging every row into Python on
    # every folder browse.
    if rel_clean:
        prefix = like_escape(f"{rel_clean}/")
        tracks_query = (
            select(Track)
            .where(Track.path.like(f"{prefix}%", escape=LIKE_ESCAPE_CHAR))
            .where(~Track.path.like(f"{prefix}%/%", escape=LIKE_ESCAPE_CHAR))
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


def all_folders(db: Session) -> list[FolderEntry]:
    """Every directory under MUSIC_DIR at any depth, with recursive track
    counts — one response, so the tree UI can build, filter and auto-reveal
    the whole hierarchy client-side instead of round-tripping per level."""
    root = music_root()

    # Recursive counts in one pass: each track increments every ancestor dir.
    counts: dict[str, int] = {}
    for track_path in db.scalars(select(Track.path)):
        parts = track_path.split("/")[:-1]
        for i in range(1, len(parts) + 1):
            prefix = "/".join(parts[:i])
            counts[prefix] = counts.get(prefix, 0) + 1

    entries: list[FolderEntry] = []

    def walk(abs_dir: Path, rel: str) -> None:
        for child in sorted(abs_dir.iterdir(), key=lambda p: p.name.lower()):
            if not child.is_dir():
                continue
            child_rel = f"{rel}/{child.name}" if rel else child.name
            entries.append(
                FolderEntry(
                    name=child.name,
                    path=child_rel,
                    track_count=counts.get(child_rel, 0),
                    has_children=any(g.is_dir() for g in child.iterdir()),
                )
            )
            walk(child, child_rel)

    walk(root, "")
    return entries


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
        prefix = like_escape(f"{rel_clean}/")
        rows = db.scalars(
            select(Track).where(
                (Track.path == rel_clean)
                | (Track.path.like(f"{prefix}%", escape=LIKE_ESCAPE_CHAR))
            )
        ).all()
        for row in rows:
            db.delete(row)
            removed_rows += 1

    if recursive:
        shutil.rmtree(target)
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
    # A case-only rename ("CD1" → "Cd1") looks like a collision on a
    # case-insensitive filesystem (Windows/macOS) but is legal — route it
    # through a temp name so the move doesn't refuse its own target.
    case_only = (
        src_clean.casefold() == dst_clean.casefold()
        and src_abs.parent == dst_abs.parent
    )
    if dst_abs.exists() and not case_only:
        raise FileExistsError(f"destination already exists: {dst_clean}")

    if case_only:
        tmp_abs = src_abs.parent / f"{src_abs.name}.cleanup-tmp"
        shutil.move(str(src_abs), str(tmp_abs))
        shutil.move(str(tmp_abs), str(dst_abs))
    else:
        dst_abs.parent.mkdir(parents=True, exist_ok=True)
        shutil.move(str(src_abs), str(dst_abs))

    rewritten = 0
    if db is not None:
        prefix = like_escape(f"{src_clean}/")
        rows = db.scalars(
            select(Track).where(
                (Track.path == src_clean)
                | (Track.path.like(f"{prefix}%", escape=LIKE_ESCAPE_CHAR))
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


__all__ = [
    "AUDIO_EXTENSIONS",
    "WRITABLE_TAGS",
    "FolderEntry",
    "ScanSummary",
    "cover_art_for",
    "delete_folder",
    "ensure_folder",
    "is_audio_file",
    "last_scan_at",
    "list_folder",
    "metadata_for",
    "music_root",
    "next_track_id_after",
    "read_file_tags",
    "relative_audio_files_in",
    "remove_path",
    "rename_folder",
    "safe_join",
    "scan_full",
    "scan_paths",
    "to_absolute",
    "to_relative",
    "track_ids_under",
    "update_path",
    "write_tags",
]
