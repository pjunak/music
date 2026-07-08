"""Cleanup engine: detect and propose fixes for filename/tag residue.

CD rips and download dumps leave predictable junk in filenames — leading
track numbers, the artist or album baked into every name, "(Official
Audio)" suffixes, underscores for spaces. This module turns a set of
indexed tracks into a *proposal*: per-track rename + tag suggestions with
old/new values, the rules that produced each, and a confidence grade. It
never touches disk or DB — the API layer presents the proposal as a
reviewable diff and applies only the operations the operator accepted.

Heuristics lean on sibling corroboration: a leading number is far more
likely a track number when most files in the folder share the pattern with
distinct values; a " - "-separated first segment shared across a folder is
almost certainly an artist/album prefix. Uncorroborated guesses still ship
but graded "low" so the UI can default-untick them.

Old values in tag suggestions are the *row* values (which mirror the file's
tags, falling back to filename/folder when a tag is absent — see
`_filename_fallbacks` in index.py). That keeps analysis DB-pure (no
per-file tag reads while scanning), and it means a suggestion only appears
when it would visibly change something — an "invisible diff" row (write
the fallback value into the file) is never proposed. Apply stale-checks
against the row too. Revert fidelity is the apply layer's job: each
journaled tag write also records the file's REAL pre-write tag value
(`file_old`, read at apply time when the file is being touched anyway), so
revert restores the true tag state — a tag that was originally absent gets
deleted again rather than materialised with the old visible value.
"""
from __future__ import annotations

import re
import unicodedata
from collections import Counter
from collections.abc import Collection, Mapping, Sequence
from dataclasses import dataclass, field
from pathlib import PurePosixPath
from typing import Protocol

# Verdict map: loose name key -> (artist_score, album_score) from cached
# MusicBrainz lookups. Fed in by the API layer; None means "offline only".
Verdicts = Mapping[str, tuple[int, int]]

# --- rule ids ---------------------------------------------------------------

RULE_NUMBERS = "strip_track_numbers"
RULE_ARTIST = "strip_artist"
RULE_ALBUM = "strip_album"
RULE_JUNK = "strip_junk"
RULE_SEPARATORS = "normalize_separators"
RULE_CASE = "normalize_case"
RULE_TAG_TITLE = "tag_title"
RULE_TAG_ARTIST = "tag_artist"
RULE_TAG_ALBUM = "tag_album"
RULE_TAG_NUMBER = "tag_number"
RULE_TAG_YEAR = "tag_year"
RULE_FOLDERS = "rename_folders"

ALL_RULES: frozenset[str] = frozenset(
    {
        RULE_NUMBERS,
        RULE_ARTIST,
        RULE_ALBUM,
        RULE_JUNK,
        RULE_SEPARATORS,
        RULE_CASE,
        RULE_TAG_TITLE,
        RULE_TAG_ARTIST,
        RULE_TAG_ALBUM,
        RULE_TAG_NUMBER,
        RULE_TAG_YEAR,
        RULE_FOLDERS,
    }
)
# Case normalisation is opinionated (ALL-CAPS stems may be deliberate), so
# it's the one rule that defaults off.
DEFAULT_RULES: frozenset[str] = ALL_RULES - {RULE_CASE}

HIGH = "high"
LOW = "low"


class TrackLike(Protocol):
    """The slice of the Track row the engine reads. Tests can pass any
    object with these attributes; the API passes ORM rows. Read-only so
    ORM descriptor attributes (`Mapped[...]`) satisfy the protocol."""

    @property
    def id(self) -> int: ...
    @property
    def path(self) -> str: ...
    @property
    def title(self) -> str: ...
    @property
    def artist(self) -> str: ...
    @property
    def album_artist(self) -> str: ...
    @property
    def album(self) -> str: ...
    @property
    def track_no(self) -> int | None: ...
    @property
    def disc_no(self) -> int | None: ...
    @property
    def year(self) -> int | None: ...


@dataclass
class Suggestion:
    track_id: int
    kind: str  # "rename" | "tag"
    field: str | None  # tag field for kind="tag"; None for renames
    old: str | int | None
    new: str | int | None
    rules: tuple[str, ...]
    confidence: str  # HIGH | LOW
    verified: bool = False  # value confirmed by an online name lookup

    @property
    def op_id(self) -> str:
        return (
            f"{self.track_id}:rename"
            if self.kind == "rename"
            else f"{self.track_id}:tag:{self.field}"
        )


@dataclass
class TrackPlan:
    track_id: int
    path: str
    ops: list[Suggestion] = field(default_factory=list)
    notes: list[str] = field(default_factory=list)
    # Names this track's analysis computed but couldn't settle (no verdict
    # yet, and not emitted as a confident op) — e.g. an album candidate
    # suppressed as an invisible diff that a lookup might flip to the
    # artist field. Feeds `pending_lookups`; not part of the visible plan.
    wants_lookup: list[str] = field(default_factory=list)


@dataclass
class FolderSuggestion:
    """A proposed folder rename (leaf-only — `path` stays put, its last
    segment becomes `new`). Folder-level, so it lives outside `TrackPlan`;
    the apply layer renames it via the index's `rename_folder` (which keeps
    every track path under it in sync) and journals it for revert."""

    path: str  # current folder path (relative, forward-slash)
    old: str  # current leaf name
    new: str  # proposed leaf name
    rules: tuple[str, ...]
    confidence: str  # HIGH | LOW

    @property
    def op_id(self) -> str:
        return f"folder:{self.path}"


# --- text helpers -----------------------------------------------------------

_WS_RE = re.compile(r"\s+")


def _fold(s: str) -> str:
    nf = unicodedata.normalize("NFKD", s)
    return "".join(c for c in nf if not unicodedata.combining(c))


def _loose(s: str) -> str:
    """Accent-folded, casefolded, alphanumeric-only comparison key."""
    return re.sub(r"[^a-z0-9]+", "", _fold(s).casefold())


def _loose_eq(a: str, b: str) -> bool:
    la, lb = _loose(a), _loose(b)
    return bool(la) and la == lb


def loose_key(name: str) -> str:
    """Public form of the comparison key — the lookup cache is keyed on it
    so every spelling variant of a verified name shares one verdict."""
    return _loose(name)


def verdict_kind(artist_score: int, album_score: int) -> str:
    """Classify a name from its MusicBrainz search scores (0-100 relevance
    of the best match as an artist vs as a release-group). Conservative on
    purpose: anything not clearly one-sided is "both"/"unknown" and changes
    nothing — a verdict may upgrade or flip a guess, never muddy a fact."""
    artist_strong, album_strong = artist_score >= 90, album_score >= 90
    if artist_strong and album_strong:
        return "both"
    if artist_strong and album_score <= 70:
        return "artist"
    if album_strong and artist_score <= 70:
        return "album"
    if artist_strong or album_strong:
        return "both"  # one strong, the other middling — too close to call
    return "unknown"


def _known_kind(name: str | None, verdicts: Verdicts | None) -> str | None:
    if not verdicts or not name:
        return None
    scores = verdicts.get(_loose(name))
    if scores is None:
        return None
    return verdict_kind(*scores)


def _normalize_separators(stem: str) -> str:
    out = stem.replace("%20", " ")
    # Underscores as space stand-ins — only when the stem has no real
    # spaces (mixed use usually means the underscores are deliberate).
    if "_" in out and " " not in out:
        out = out.replace("_", " ")
    return _WS_RE.sub(" ", out).strip()


def _tidy(stem: str) -> str:
    """Final cleanup after transforms: collapse whitespace, drop empty
    bracket pairs, trim dangling separators at both ends."""
    out = re.sub(r"\(\s*\)|\[\s*\]", "", stem)
    out = _WS_RE.sub(" ", out).strip()
    return out.strip(" -–—._")


def _smart_title(stem: str) -> str:
    """Word-capitalise without str.title()'s apostrophe mangling
    ("don't" → "Don't", not "Don'T")."""
    return " ".join(w[:1].upper() + w[1:].lower() if w else w for w in stem.split(" "))


# --- leading track numbers ---------------------------------------------------

# "01 - Title", "01. Title", "01_Title", "[01] - Title", "1-01 - Title".
# 1-3 digits only: "2017 - Song" is a year, not a track number.
_NUM_SEP_RE = re.compile(r"^[(\[]?(\d{1,3})(?:[-.](\d{1,2}))?[)\]]?\s*[-–—._]+\s*")
# "01 Title", "(01) Title" — bare number, no separator. Much weaker signal.
_NUM_BARE_RE = re.compile(r"^[(\[]?(\d{1,3})(?:[-.](\d{1,2}))?[)\]]?\s+")


@dataclass(frozen=True)
class _NumberMatch:
    track: int | None
    disc: int | None
    rest: str
    strong: bool  # dash-separated form — unambiguous even without siblings


def _match_leading_number(stem: str) -> _NumberMatch | None:
    m = _NUM_SEP_RE.match(stem) or _NUM_BARE_RE.match(stem)
    if m is None:
        return None
    rest = stem[m.end() :]
    if not rest.strip():
        return None  # the whole stem is the number ("1979.mp3") — keep it
    g1, g2 = m.group(1), m.group(2)
    if g2 is not None:
        # "1-01" → disc 1 track 1. A first number above 9 isn't a disc
        # count; treat the pair as opaque (still strippable, no tag).
        disc, track = (int(g1), int(g2)) if int(g1) <= 9 else (None, None)
    else:
        disc, track = None, int(g1)
    strong = any(c in m.group(0) for c in "-–—")
    return _NumberMatch(track=track, disc=disc, rest=rest, strong=strong)


# --- " - " segments ----------------------------------------------------------

# Split only where the dash has whitespace on at least one side (or the
# underscore form): "Artist - Title" splits, "Spider-Man" doesn't.
_SEG_SPLIT = re.compile(r"\s+-\s+|\s+-(?=\S)|(?<=\S)-\s+|_-_")


def _segments(stem: str) -> list[str]:
    return [p for p in _SEG_SPLIT.split(stem) if p.strip()]


# --- junk phrases ------------------------------------------------------------

_JUNK_INNER = re.compile(
    r"""^(?:
        (?:official\s+)?(?:music\s+|lyric s?\s+)?(?:audio|video|visuali[sz]er)
        |official
        |lyrics?
        |with\s+lyrics?
        |audio\s+only
        |official\s+(?:song|track)
        |(?:hq|hd)\s+(?:audio|video|version)
        |hd|hq|4k
        |full\s+(?:album|song|track)
        |free\s+download
        |downloaded\s+from\b.*
        |youtube(?:\.com)?
        |\d{2,4}\s*kbps|\d{2,4}\s*kb/?s|cbr|vbr\s*v?\d?
    )$""",
    re.IGNORECASE | re.VERBOSE,
)
_JUNK_DOMAIN = re.compile(
    r"(?:www\.)?[a-z0-9][a-z0-9.-]*\.(?:com|net|org|ru|info|me|cc|to|io|biz|pl|cz|sk|fm|co)\b",
    re.IGNORECASE,
)
_BRACKET_GROUP = re.compile(r"[(\[][^()\[\]]*[)\]]")
_TRAILING_DASH_JUNK = re.compile(
    r"[-–—]\s*(?:official(?:\s+(?:audio|video|music\s+video))?|lyrics?|audio|youtube)\s*$",
    re.IGNORECASE,
)
_EDGE_SITE = re.compile(
    r"^(?:www\.)?[a-z0-9.-]+\.(?:com|net|org|ru|info|me|cc|to|io|biz|pl|cz|sk|fm|co)[\s_–—-]+"
    r"|[\s_–—-]+(?:www\.)?[a-z0-9.-]+\.(?:com|net|org|ru|info|me|cc|to|io|biz|pl|cz|sk|fm|co)$",
    re.IGNORECASE,
)


def _is_junk_group(inner: str) -> bool:
    inner = inner.strip()
    return bool(_JUNK_INNER.match(inner)) or bool(_JUNK_DOMAIN.search(inner))


def _strip_junk(stem: str) -> str:
    def drop_junk_groups(s: str) -> str:
        return _BRACKET_GROUP.sub(
            lambda m: "" if _is_junk_group(m.group(0)[1:-1]) else m.group(0), s
        )

    out = drop_junk_groups(stem)
    out = _EDGE_SITE.sub("", out)
    out = _TRAILING_DASH_JUNK.sub("", out)
    return out


# --- folder context ----------------------------------------------------------

# Folder names that describe storage, not music — never offered as an
# artist/album value (a file in "Uploads/" is not on an album called
# Uploads). Matched loosely (case/accents/punctuation ignored).
_GENERIC_FOLDER_NAMES: frozenset[str] = frozenset(
    _loose(n)
    for n in (
        "uploads", "upload", "music", "new", "misc", "various", "downloads",
        "download", "import", "imports", "inbox", "unsorted", "sorted",
        "tracks", "songs", "audio", "files", "library", "collection",
        "collections", "mp3", "mp3s", "flac", "flacs", "temp", "tmp", "other",
        "stuff", "mixed", "mix", "singles", "albums", "album", "artists",
        "artist", "compilations", "compilation", "playlists", "playlist",
        "soundtracks", "soundtrack", "ost", "music library", "random",
    )
)


def _is_generic_name(name: str) -> bool:
    return not name or _loose(name) in _GENERIC_FOLDER_NAMES


# "CD1", "Disc 2", "disk_3" — a disc container inside an album folder.
_DISC_FOLDER_RE = re.compile(r"(?:cd|disc|disk)[\s._-]*(\d{1,2})", re.IGNORECASE)
# "Part 1", "pt2", "Part.3" — a part container (used like a disc). `fullmatch`
# at the call sites keeps real titles ("Part of Me", "Particle") out.
_PART_FOLDER_RE = re.compile(r"(?:pt|part)[\s._-]*(\d{1,3})", re.IGNORECASE)
# A folder named only as a number ("1", "01") carries no album identity —
# a rebuild-from-tags trigger.
_PURE_NUM_RE = re.compile(r"^\d+$")
# Year decoration on a folder name: "Album (2013)" / "[1987]" / "2019 - Album".
_FOLDER_YEAR_RE = re.compile(
    r"^\s*((?:19|20)\d{2})\s*[-–—.]\s*(.+)$|^(.+?)\s*[(\[]((?:19|20)\d{2})[)\]]\s*$"
)


def _split_folder_label(name: str) -> tuple[str | None, str, int | None]:
    """Decompose a folder name into (artist_part, album_part, year).

    "Artist - Album (2013)" → ("Artist", "Album", 2013); "2019 - Album" →
    (None... or split; "Plain Name" → (None, "Plain Name", None). The split
    only fires on a spaced " - " so hyphenated names stay whole.

    Separators are normalised first (`Skyrim_OST_(2011)` → `Skyrim OST
    (2011)`) so the artist/album values this yields — which become both tag
    suggestions and the folder-rename target — are already tidy."""
    base = _normalize_separators(name)
    year: int | None = None
    m = _FOLDER_YEAR_RE.match(base)
    if m:
        if m.group(1) is not None:
            year, base = int(m.group(1)), m.group(2).strip()
        else:
            base, year = m.group(3).strip(), int(m.group(4))
    parts = [p.strip() for p in re.split(r"\s+-\s+", base) if p.strip()]
    if len(parts) == 2:
        return parts[0], parts[1], year
    return None, base, year


def _dominant(values: list[str]) -> tuple[str | None, bool]:
    """(most common non-empty value, was it unanimous) when at least two
    values agree and they make up ≥ 2/3 of the non-empty set — the
    partially-tagged-album signal. (None, False) otherwise."""
    cleaned = [v for v in values if v]
    if len(cleaned) < 2:
        return None, False
    counts = Counter(_loose(v) for v in cleaned)
    key, count = counts.most_common(1)[0]
    if not key or count < 2 or count / len(cleaned) < 2 / 3:
        return None, False
    texts = Counter(v for v in cleaned if _loose(v) == key)
    return texts.most_common(1)[0][0], count == len(cleaned)


@dataclass
class _FolderCtx:
    name: str  # effective album-folder leaf ("" at root; the parent of a CD1/ folder)
    parent_name: str  # raw immediate parent leaf (may be the disc folder itself)
    grandparent: str  # leaf above the effective album folder ("" when none)
    numbers_corroborated: bool
    seg1: str | None  # canonical text of a folder-wide shared first segment
    seg2: str | None
    disc_from_folder: int | None
    folder_artist: str | None  # "Artist" half of an "Artist - Album" folder name
    folder_album: str | None  # album candidate from the folder name (year/artist stripped)
    folder_year: int | None
    albumish: bool  # the folder smells like one album's container
    dominant_artist: str | None
    dominant_artist_unanimous: bool
    dominant_album: str | None
    dominant_album_unanimous: bool
    verdicts: Verdicts | None = None


def _numbers_corroborated(stems: Sequence[str]) -> bool:
    matches = [m for m in (_match_leading_number(s) for s in stems) if m is not None]
    if len(matches) < 2 or len(matches) / len(stems) < 0.6:
        return False
    nums = [m.track for m in matches if m.track is not None]
    if len(nums) < 2:
        return False
    # Mostly-distinct values — "01, 02, 03" corroborates; "01, 01, 01"
    # is more likely a series title than track numbers.
    return len(set(nums)) / len(nums) >= 0.8


def _shared_segment(seg_lists: Sequence[list[str]], index: int) -> str | None:
    """Canonical text of the segment most files share at `index`, requiring
    a remainder after it (a shared prefix that *is* the whole name isn't a
    prefix). None when fewer than 80% (min 2) share it."""
    present = [parts[index] for parts in seg_lists if len(parts) > index + 1]
    if len(present) < 2:
        return None
    counts = Counter(_loose(p) for p in present)
    key, count = counts.most_common(1)[0]
    if not key or count < 2 or count < 0.8 * len(seg_lists):
        return None
    texts = Counter(p for p in present if _loose(p) == key)
    return texts.most_common(1)[0][0]


def _build_ctx(
    folder: str, tracks: Sequence[TrackLike], verdicts: Verdicts | None = None
) -> _FolderCtx:
    path_parts = PurePosixPath(folder).parts if folder else ()
    parent_name = path_parts[-1] if path_parts else ""

    # A "CD1"-style parent is a disc container: the *album* context is one
    # level up, and the disc number itself becomes a tag clue.
    disc: int | None = None
    album_parts = path_parts
    if parent_name and (dm := _DISC_FOLDER_RE.fullmatch(parent_name.strip())):
        disc = int(dm.group(1))
        album_parts = path_parts[:-1]
    name = album_parts[-1] if album_parts else ""
    grandparent = album_parts[-2] if len(album_parts) >= 2 else ""

    folder_artist: str | None = None
    folder_album: str | None = None
    folder_year: int | None = None
    if name:
        folder_artist, folder_album, folder_year = _split_folder_label(name)

    stems = [_normalize_separators(PurePosixPath(t.path).stem) for t in tracks]

    pass1 = _numbers_corroborated(stems)
    denumbered = [(m.rest if (m := _match_leading_number(s)) else s) for s in stems]

    seg_lists = [_segments(s) for s in denumbered]
    seg1 = _shared_segment(seg_lists, 0)
    seg2: str | None = None
    if seg1 is not None:
        sharing = [parts for parts in seg_lists if parts and _loose_eq(parts[0], seg1)]
        seg2 = _shared_segment([parts[1:] for parts in sharing], 0)

    # Second corroboration pass on the segment-stripped forms catches the
    # "Artist - 01 - Title" layout where the number isn't at position 0.
    pass2 = False
    if seg1 is not None:
        stripped = []
        for parts in seg_lists:
            n_drop = 1 if (parts and _loose_eq(parts[0], seg1)) else 0
            if n_drop and seg2 is not None and len(parts) > 2 and _loose_eq(parts[1], seg2):
                n_drop = 2
            stripped.append(" - ".join(parts[n_drop:]))
        pass2 = _numbers_corroborated(stripped)

    # Sibling-tag dominance: a partially-tagged album lets the tagged
    # majority fill the stragglers. Values that are (probably) not real
    # tags don't vote: disc-folder fallbacks ("CD1"), suspect duplicates
    # (album == artist, a common rip bug), and folder-name echoes — an
    # untagged row's album mirrors its parent folder, so counting those
    # would let fallbacks outvote the genuinely tagged siblings.
    dominant_artist, da_unanimous = _dominant([t.artist for t in tracks])
    dominant_album, dal_unanimous = _dominant(
        [
            t.album
            for t in tracks
            if t.album
            and not _DISC_FOLDER_RE.fullmatch(t.album.strip())
            and not (t.artist and _loose_eq(t.album, t.artist))
            and not _loose_eq(t.album, name)
            and not _loose_eq(t.album, parent_name)
        ]
    )

    return _FolderCtx(
        name=name,
        parent_name=parent_name,
        grandparent=grandparent,
        numbers_corroborated=pass1 or pass2,
        seg1=seg1,
        seg2=seg2,
        disc_from_folder=disc,
        folder_artist=folder_artist,
        folder_album=folder_album,
        folder_year=folder_year,
        albumish=folder_year is not None or disc is not None or pass1 or pass2,
        dominant_artist=dominant_artist,
        dominant_artist_unanimous=da_unanimous,
        dominant_album=dominant_album,
        dominant_album_unanimous=dal_unanimous,
        verdicts=verdicts,
    )


# --- per-track analysis -------------------------------------------------------


@dataclass
class _Extraction:
    track_no: int | None = None
    disc_no: int | None = None
    number_conf: str = LOW
    artist: str | None = None
    artist_conf: str = LOW
    album: str | None = None
    album_conf: str = LOW


def _album_emptyish(track: TrackLike, ctx: _FolderCtx) -> bool:
    """True when the album tag carries no usable information: absent, a
    filename/folder fallback (the row mirrors the parent folder name when
    the tag is missing), or a *suspect duplicate* — rips commonly stuff the
    artist's name into the album field, so album == artist means the real
    album is still unknown and folder clues stay in play."""
    if not track.album:
        return True
    if _loose_eq(track.album, ctx.name) or _loose_eq(track.album, ctx.parent_name):
        return True
    if track.artist and _loose_eq(track.album, track.artist):
        return True
    return bool(track.album_artist) and _loose_eq(track.album, track.album_artist)


def _classify_segment(
    seg: str, position: int, n_parts: int, track: TrackLike, ctx: _FolderCtx
) -> tuple[str, str, tuple[str, str, str] | None] | None:
    """Decide whether a leading " - " segment is redundant metadata.

    Returns (strip_rule, confidence, tag_guess) where tag_guess is
    (field, value, confidence) or None — or None when the segment looks
    like part of the actual title and must be kept.
    """
    artist_empty = not track.artist
    album_emptyish = _album_emptyish(track, ctx)

    if _loose_eq(seg, track.artist) or _loose_eq(seg, track.album_artist):
        return (RULE_ARTIST, HIGH, None)
    if _loose_eq(seg, track.album) and not album_emptyish:
        # A suspect album value (== artist) was already caught by the
        # artist branch above; a folder-fallback value is handled below.
        return (RULE_ALBUM, HIGH, None)
    if ctx.folder_artist is not None and _loose_eq(seg, ctx.folder_artist):
        # The folder spells it out: "Artist - Album". A segment matching
        # the artist half is the artist, with structural certainty.
        return (RULE_ARTIST, HIGH, ("artist", seg, HIGH) if artist_empty else None)
    if (
        ctx.folder_artist is not None
        and ctx.folder_album is not None
        and _loose_eq(seg, ctx.folder_album)
    ):
        return (RULE_ALBUM, HIGH, ("album", seg, HIGH) if album_emptyish else None)
    if _loose_eq(seg, ctx.grandparent):
        # Artist/Album/"Artist - Title.mp3" — the segment names the
        # grandparent folder, the classic per-artist library layout.
        return (RULE_ARTIST, HIGH, ("artist", seg, HIGH) if artist_empty else None)
    if _loose_eq(seg, ctx.name) or (
        ctx.folder_album is not None and _loose_eq(seg, ctx.folder_album)
    ):
        # Folder named after the segment (possibly modulo a year marker).
        # Stripping is safe — the segment is folder-redundant either way —
        # but whether that name is an artist or an album is genuinely
        # ambiguous (a game-soundtrack folder is an album-ish name, a band
        # folder is an artist). An online verdict settles it; otherwise the
        # tag guess ships low.
        kind = _known_kind(seg, ctx.verdicts)
        if kind == "album":
            return (RULE_ALBUM, HIGH, ("album", seg, HIGH) if album_emptyish else None)
        guess_conf = HIGH if kind == "artist" else LOW
        return (RULE_ARTIST, HIGH, ("artist", seg, guess_conf) if artist_empty else None)
    if position == 0 and ctx.seg1 is not None and _loose_eq(seg, ctx.seg1):
        kind = _known_kind(seg, ctx.verdicts)
        if kind == "album" and album_emptyish:
            # A folder-wide "Album - Title" prefix, confirmed by lookup.
            return (RULE_ALBUM, HIGH, ("album", seg, HIGH))
        conf = HIGH if kind == "artist" else LOW
        return (RULE_ARTIST, HIGH, ("artist", seg, conf) if artist_empty else None)
    if position == 1 and ctx.seg2 is not None and _loose_eq(seg, ctx.seg2):
        conf = HIGH if _known_kind(seg, ctx.verdicts) == "album" else LOW
        return (RULE_ALBUM, HIGH, ("album", seg, conf) if album_emptyish else None)
    if (
        position == 0
        and ctx.seg1 is None
        and n_parts == 2
        and not seg.strip().isdigit()  # "01 - Foo" with number-strip off isn't "artist 01"
    ):
        # Lone "A - B" file with no siblings to corroborate. A verdict can
        # settle which half of the metadata the prefix is; otherwise it's
        # most likely "Artist - Title", but only as a low guess.
        kind = _known_kind(seg, ctx.verdicts)
        if kind == "album" and album_emptyish:
            return (RULE_ALBUM, HIGH, ("album", seg, HIGH))
        if artist_empty:
            if kind == "artist":
                return (RULE_ARTIST, HIGH, ("artist", seg, HIGH))
            return (RULE_ARTIST, LOW, ("artist", seg, LOW))
    return None


def _transform_stem(
    stem0: str,
    track: TrackLike,
    ctx: _FolderCtx,
    enabled: Collection[str],
    extraction: _Extraction,
) -> tuple[str, dict[str, str]]:
    """Run the enabled filename transforms over `stem0`. Returns the new
    stem and {rule: confidence} for every transform that changed it.
    Extraction side-products (track number, artist/album guesses) are
    recorded on `extraction` regardless of which strip rules are enabled —
    tag suggestions don't depend on the rename being wanted."""
    fired: dict[str, str] = {}

    def fire(rule: str, conf: str) -> None:
        fired[rule] = LOW if fired.get(rule) == LOW or conf == LOW else conf

    stem = stem0
    normalized = _normalize_separators(stem)
    if RULE_SEPARATORS in enabled and normalized != stem:
        fire(RULE_SEPARATORS, HIGH)
        stem = normalized

    def consume_number(s: str) -> str:
        m = _match_leading_number(_normalize_separators(s) if RULE_SEPARATORS not in enabled else s)
        # Match against the live stem too so a disabled separator rule
        # doesn't block stripping (the regexes treat _ as a separator).
        m_live = _match_leading_number(s)
        chosen = m_live or m
        if chosen is None:
            return s
        strip_conf = HIGH if (ctx.numbers_corroborated or chosen.strong) else LOW
        if extraction.track_no is None and chosen.track is not None:
            extraction.track_no = chosen.track
            extraction.disc_no = chosen.disc
            # Stripping and tagging are different-strength claims: a lone
            # "05 - Foo" is confidently strippable junk (dash form), but
            # only a sibling sequence (01, 02, 03…) justifies asserting
            # it's *track 5* — a folder of unrelated "01 - X" singles, or
            # duplicated numbers, shouldn't all get tagged track 1.
            extraction.number_conf = HIGH if ctx.numbers_corroborated else LOW
        if RULE_NUMBERS not in enabled or m_live is None:
            return s
        fire(RULE_NUMBERS, strip_conf)
        return m_live.rest

    stem = consume_number(stem)

    # Leading " - " segments: artist / album prefixes.
    parts = _segments(stem)
    drop = 0
    for position in range(2):
        if len(parts) - drop < 2 or position >= len(parts):
            break
        verdict = _classify_segment(parts[position], position, len(parts), track, ctx)
        if verdict is None:
            break
        rule, conf, guess = verdict
        if guess is not None:
            g_field, g_value, g_conf = guess
            if g_field == "artist" and extraction.artist is None:
                extraction.artist = g_value
                extraction.artist_conf = g_conf
            elif g_field == "album" and extraction.album is None:
                extraction.album = g_value
                extraction.album_conf = g_conf
        if rule in enabled:
            drop += 1
            fire(rule, conf)
        elif position == 0:
            # Can't strip seg 0, so seg 1 isn't leading any more.
            break
    if drop:
        stem = " - ".join(parts[drop:])
        # The prefix may have hidden a track number ("Artist - 01 - Title").
        stem = consume_number(stem)

    if RULE_JUNK in enabled:
        dejunked = _strip_junk(stem)
        if dejunked != stem:
            fire(RULE_JUNK, HIGH)
            stem = dejunked

    if RULE_CASE in enabled and stem and (stem.isupper() or stem.islower()):
        recased = _smart_title(stem)
        if recased != stem:
            fire(RULE_CASE, HIGH)
            stem = recased

    if fired:
        stem = _tidy(stem)
    if not stem:
        # Transforms ate the whole name — abort the rename, keep extractions.
        return stem0, {}
    return stem, fired


def _worst(confs: Collection[str]) -> str:
    return LOW if LOW in confs else HIGH


def _plan_track(track: TrackLike, ctx: _FolderCtx, enabled: Collection[str]) -> TrackPlan:
    """Tag ops are emitted BEFORE the rename op, and apply preserves order:
    for an untagged file the row's title mirrors the filename stem, so the
    rename must land *after* the title write or the title op's stale-check
    (old == pre-rename stem) would false-trip."""
    plan = TrackPlan(track_id=track.id, path=track.path)
    ppath = PurePosixPath(track.path)
    extraction = _Extraction()

    new_stem, fired = _transform_stem(ppath.stem, track, ctx, enabled, extraction)

    if RULE_TAG_TITLE in enabled:
        # Clean the title tag with the full pipeline (a junky title tag has
        # the same residue as a junky filename, often because the ripper
        # set title := filename).
        t_extraction = _Extraction()
        new_title, t_fired = _transform_stem(
            track.title, track, ctx, ALL_RULES if RULE_CASE in enabled else DEFAULT_RULES, t_extraction
        )
        if t_fired and new_title and new_title != track.title:
            plan.ops.append(
                Suggestion(
                    track_id=track.id,
                    kind="tag",
                    field="title",
                    old=track.title,
                    new=new_title,
                    rules=(RULE_TAG_TITLE,),
                    confidence=_worst(t_fired.values()),
                )
            )

    artist_allowed = RULE_TAG_ARTIST in enabled and not track.artist
    album_allowed = RULE_TAG_ALBUM in enabled and _album_emptyish(track, ctx)

    # Each candidate carries whether a contrary online verdict may *flip*
    # it to the other field: yes for structure/filename guesses (a folder
    # name's kind is exactly what's uncertain), never for values copied
    # from real tags (album_artist, sibling-tag dominance) — a fuzzy name
    # match doesn't outrank explicit metadata.
    artist_new: str | None = None
    artist_conf = LOW
    artist_flippable = False
    if artist_allowed:
        # Strongest signal first: the file's own album-artist tag, then
        # what the filename segments said, then folder-level clues.
        if track.album_artist:
            artist_new, artist_conf = track.album_artist, HIGH
        elif extraction.artist:
            artist_new, artist_conf = extraction.artist, extraction.artist_conf
            artist_flippable = extraction.artist_conf == LOW
        elif ctx.dominant_artist:
            # Tagged siblings vote: a partially-tagged album fills its
            # stragglers; high only when every tagged sibling agrees.
            artist_new = ctx.dominant_artist
            artist_conf = HIGH if ctx.dominant_artist_unanimous else LOW
        elif ctx.folder_artist and not _is_generic_name(ctx.folder_artist):
            artist_new, artist_flippable = ctx.folder_artist, True
        elif ctx.albumish and not _is_generic_name(ctx.grandparent):
            # Artist/Album/tracks layout: the level above an album-looking
            # folder is usually the artist — but only a guess.
            artist_new, artist_flippable = ctx.grandparent, True

    album_new: str | None = None
    album_conf = LOW
    album_flippable = False
    if album_allowed:
        artistish = [
            v
            for v in (
                track.artist,
                track.album_artist,
                extraction.artist,
                artist_new,
                ctx.dominant_artist,
                ctx.folder_artist,
            )
            if v
        ]
        if extraction.album:
            album_new = extraction.album
            album_conf = (
                HIGH
                if ctx.folder_album and _loose_eq(extraction.album, ctx.folder_album)
                else extraction.album_conf
            )
            album_flippable = album_conf == LOW
        elif ctx.dominant_album:
            album_new = ctx.dominant_album
            album_conf = HIGH if ctx.dominant_album_unanimous else LOW
        elif (
            ctx.folder_album
            and not _is_generic_name(ctx.folder_album)
            and not any(_loose_eq(ctx.folder_album, a) for a in artistish)
        ):
            # The folder itself, stripped of year decoration / artist half.
            # High when the folder smells like one album's container
            # (year marker, disc subfolders, a numbered sequence) — but the
            # *kind* of the name stays uncertain, so it remains flippable.
            album_new = ctx.folder_album
            album_conf = HIGH if ctx.albumish else LOW
            album_flippable = True

    # Online verdicts referee the candidates: a confirmed kind upgrades the
    # guess; a *contradicted* flippable one swaps sides instead of shipping
    # wrong — e.g. a folder named after the artist would offer it as the
    # album, but a lookup saying "that's an artist" moves it there.
    verdict_artist = _known_kind(artist_new, ctx.verdicts)
    verdict_album = _known_kind(album_new, ctx.verdicts)
    artist_conf_pre, album_conf_pre = artist_conf, album_conf
    flip_artist = verdict_artist == "album" and artist_flippable
    flip_album = verdict_album == "artist" and album_flippable
    if verdict_artist == "artist":
        artist_conf = HIGH
    if verdict_album == "album":
        album_conf = HIGH
    swap_artist, swap_album = artist_new, album_new
    if flip_artist:
        artist_new = None
    if flip_album:
        album_new = None
    if flip_album and artist_allowed and artist_new is None and swap_album:
        artist_new, artist_conf = swap_album, HIGH
    if flip_artist and album_allowed and album_new is None and swap_artist:
        album_new, album_conf = swap_artist, HIGH

    # Names a lookup could still settle: computed candidates with no
    # verdict yet, unless they're tag-derived certainties (a lookup can
    # neither upgrade nor flip those). Includes candidates suppressed as
    # invisible diffs — settling their kind is how e.g. an artist-named
    # folder's name finds its way to the artist field on the next pass.
    for candidate, conf, flippable in (
        (swap_artist, artist_conf_pre, artist_flippable),
        (swap_album, album_conf_pre, album_flippable),
    ):
        if (
            candidate
            and (flippable or conf == LOW)
            and _known_kind(candidate, ctx.verdicts) is None
            and str(candidate) not in plan.wants_lookup
        ):
            plan.wants_lookup.append(str(candidate))

    if artist_new:
        plan.ops.append(
            Suggestion(
                track_id=track.id,
                kind="tag",
                field="artist",
                old="",
                new=artist_new,
                rules=(RULE_TAG_ARTIST,),
                confidence=artist_conf,
                verified=_known_kind(artist_new, ctx.verdicts) == "artist",
            )
        )

    if album_new and album_new != track.album:
        plan.ops.append(
            Suggestion(
                track_id=track.id,
                kind="tag",
                field="album",
                old=track.album,
                new=album_new,
                rules=(RULE_TAG_ALBUM,),
                confidence=album_conf,
                verified=_known_kind(album_new, ctx.verdicts) == "album",
            )
        )

    if RULE_TAG_NUMBER in enabled:
        if track.track_no is None and extraction.track_no is not None:
            plan.ops.append(
                Suggestion(
                    track_id=track.id,
                    kind="tag",
                    field="track_no",
                    old=None,
                    new=extraction.track_no,
                    rules=(RULE_TAG_NUMBER,),
                    confidence=extraction.number_conf,
                )
            )
        disc_new: int | None = None
        disc_conf = LOW
        if extraction.disc_no is not None:
            disc_new, disc_conf = extraction.disc_no, extraction.number_conf
        elif ctx.disc_from_folder is not None:
            # Sitting inside "CD2/" *is* the disc number — unambiguous.
            disc_new, disc_conf = ctx.disc_from_folder, HIGH
        if track.disc_no is None and disc_new is not None:
            plan.ops.append(
                Suggestion(
                    track_id=track.id,
                    kind="tag",
                    field="disc_no",
                    old=None,
                    new=disc_new,
                    rules=(RULE_TAG_NUMBER,),
                    confidence=disc_conf,
                )
            )

    if RULE_TAG_YEAR in enabled and track.year is None and ctx.folder_year is not None:
        plan.ops.append(
            Suggestion(
                track_id=track.id,
                kind="tag",
                field="year",
                old=None,
                new=ctx.folder_year,
                rules=(RULE_TAG_YEAR,),
                confidence=HIGH,
            )
        )

    if new_stem != ppath.stem and fired:
        plan.ops.append(
            Suggestion(
                track_id=track.id,
                kind="rename",
                field=None,
                old=ppath.stem,
                new=new_stem,
                rules=tuple(sorted(fired)),
                confidence=_worst(fired.values()),
            )
        )

    return plan


# --- entry point --------------------------------------------------------------


def _parent(path: str) -> str:
    parent = PurePosixPath(path).parent
    return "" if str(parent) == "." else str(parent)


def _group_by_folder(tracks: Sequence[TrackLike]) -> dict[str, list[TrackLike]]:
    by_folder: dict[str, list[TrackLike]] = {}
    for t in tracks:
        by_folder.setdefault(_parent(t.path), []).append(t)
    return by_folder


def analyze(
    scope_tracks: Sequence[TrackLike],
    all_tracks: Sequence[TrackLike],
    enabled: Collection[str] | None = None,
    verdicts: Verdicts | None = None,
) -> list[TrackPlan]:
    """Build cleanup plans for `scope_tracks`. `all_tracks` supplies folder
    context (sibling corroboration) — pass every indexed track; tracks
    outside the scope inform the heuristics but get no suggestions.
    `verdicts` is the cached online-lookup map (see `pending_lookups`);
    analysis is identical and fully offline without it, just less sure."""
    rules = frozenset(enabled) if enabled is not None else DEFAULT_RULES

    by_folder_all = _group_by_folder(all_tracks)
    by_folder_scope = _group_by_folder(scope_tracks)

    plans: list[TrackPlan] = []
    for folder, group in sorted(by_folder_scope.items()):
        # Corroborate within the scoped subset when it's big enough to mean
        # something (an album selected inside a junk-drawer folder), else
        # over the whole folder.
        ctx_tracks = group if len(group) >= 2 else by_folder_all.get(folder, group)
        ctx = _build_ctx(folder, ctx_tracks, verdicts)

        folder_plans = [_plan_track(t, ctx, rules) for t in sorted(group, key=lambda t: t.path)]

        # Collision pass: a proposed name must not hit an existing sibling
        # file or another proposed name (case-insensitive — the deploy
        # target may be case-insensitive and Windows dev certainly is).
        existing = {
            PurePosixPath(t.path).name.casefold(): t.id
            for t in by_folder_all.get(folder, group)
        }
        proposed: dict[str, int] = {}
        for plan in folder_plans:
            rename = next((o for o in plan.ops if o.kind == "rename"), None)
            if rename is None:
                continue
            suffix = PurePosixPath(plan.path).suffix
            key = f"{rename.new}{suffix}".casefold()
            clash_existing = existing.get(key)
            if clash_existing is not None and clash_existing != plan.track_id:
                plan.ops.remove(rename)
                plan.notes.append(
                    f'rename dropped: "{rename.new}{suffix}" already exists in this folder'
                )
                continue
            if key in proposed:
                plan.ops.remove(rename)
                plan.notes.append(
                    f'rename dropped: another track would also become "{rename.new}{suffix}"'
                )
                continue
            proposed[key] = plan.track_id

        plans.extend(p for p in folder_plans if p.ops or p.notes or p.wants_lookup)

    return plans


# --- folder renames -----------------------------------------------------------


def _sanitize_leaf(name: str) -> str:
    """A folder leaf can't hold a path separator; collapse whitespace and
    trim dangling separators, like `_tidy` does for filenames."""
    out = name.replace("/", "-").replace("\\", "-")
    out = _WS_RE.sub(" ", out).strip()
    return out.strip(" -–—._")[:120]


def _disc_part_canonical(leaf: str) -> tuple[str, str] | None:
    """`(canonical_name, rule)` when the whole leaf is a disc/part marker —
    `cd1`/`Disc 2`/`disk_3` → `Disc N`, `pt1`/`Part.2` → `Part N`. The
    spelled-out form is the "proper" one and stays detectable (so a second
    pass is a no-op). None for anything that isn't purely such a marker."""
    s = leaf.strip()
    m = _DISC_FOLDER_RE.fullmatch(s)
    if m:
        return f"Disc {int(m.group(1))}", "disc_canonical"
    m = _PART_FOLDER_RE.fullmatch(s)
    if m:
        return f"Part {int(m.group(1))}", "part_canonical"
    return None


def _tidy_folder_leaf(leaf: str, *, case: bool) -> tuple[str, tuple[str, ...]]:
    """Clean a folder leaf the way `_transform_stem` cleans a filename, minus
    the track-number / artist-segment stripping (a folder name *is* the
    album/artist — there's nothing redundant to strip). Returns the new leaf
    and the rule tags that fired."""
    fired: list[str] = []
    out = leaf
    norm = _normalize_separators(out)
    if norm != out:
        fired.append(RULE_SEPARATORS)
        out = norm
    dejunked = _strip_junk(out)
    if dejunked != out:
        fired.append(RULE_JUNK)
        out = dejunked
    if case and out and (out.isupper() or out.islower()):
        recased = _smart_title(out)
        if recased != out:
            fired.append(RULE_CASE)
            out = recased
    if fired:
        out = _tidy(out)
    return _sanitize_leaf(out), tuple(fired)


@dataclass
class _FolderClues:
    album: str | None
    artist: str | None
    year: int | None


def _single_or_dominant(values: list[str]) -> str | None:
    cleaned = [v for v in values if v]
    if not cleaned:
        return None
    if len(cleaned) == 1:
        return cleaned[0]
    return _dominant(cleaned)[0]


def _folder_clues(folder: str, under: Sequence[TrackLike]) -> _FolderClues:
    """The album/artist/year the tracks *under* a folder agree on — the raw
    material for rebuilding an unusable folder name. Album candidates exclude
    the same not-a-real-album values `_build_ctx` filters (disc/part folder
    fallbacks, album == artist, album == this folder's name)."""
    leaf = PurePosixPath(folder).name
    album_candidates = [
        t.album
        for t in under
        if t.album
        and not _DISC_FOLDER_RE.fullmatch(t.album.strip())
        and not _PART_FOLDER_RE.fullmatch(t.album.strip())
        and not (t.artist and _loose_eq(t.album, t.artist))
        and not _loose_eq(t.album, leaf)
    ]
    years = [t.year for t in under if t.year]
    return _FolderClues(
        album=_single_or_dominant(album_candidates),
        artist=_single_or_dominant([t.artist for t in under if t.artist]),
        year=years[0] if years and len(set(years)) == 1 else None,
    )


def _folder_name_usable(name: str, clues: _FolderClues) -> bool:
    """Whether a (tidied) folder name is a real album identity. A storage
    name, a bare number, or just the artist's name carries none — those are
    the cases the user asked to rebuild from tags."""
    if not name or _is_generic_name(name):
        return False
    if _PURE_NUM_RE.match(name.strip()):
        return False
    if len(_loose(name)) < 2:
        return False
    return not (clues.artist and _loose_eq(name, clues.artist))


def _rebuild_folder_name(folder: str, clues: _FolderClues) -> str | None:
    """A canonical `Album (Year)` / `Artist - Album (Year)` from the tracks'
    shared tags. The artist is prepended only when it adds information — not
    when it equals the album or is already the parent folder (an
    Artist/Album layout doesn't want `Artist/Artist - Album`)."""
    if not clues.album:
        return None
    name = clues.album
    if clues.year:
        name = f"{name} ({clues.year})"
    parent_leaf = PurePosixPath(_parent(folder)).name if _parent(folder) else ""
    if (
        clues.artist
        and not _loose_eq(clues.artist, clues.album)
        and not _loose_eq(clues.artist, parent_leaf)
    ):
        name = f"{clues.artist} - {name}"
    return _sanitize_leaf(name) or None


def _plan_folder(
    folder: str, under: Sequence[TrackLike], *, case: bool
) -> FolderSuggestion | None:
    leaf = PurePosixPath(folder).name
    if not leaf:
        return None

    # Disc / part containers get the canonical spelled-out form. Structural,
    # so high confidence — and checked before the generic guard (a disc
    # folder is never "generic") and before any text tidy.
    canon = _disc_part_canonical(leaf)
    if canon is not None:
        new, rule = canon
        return None if new == leaf else FolderSuggestion(folder, leaf, new, (rule,), HIGH)

    # Storage folders (Uploads, Music, Singles, …) are never renamed.
    if _is_generic_name(leaf):
        return None

    clues = _folder_clues(folder, under)
    tidied, fired = _tidy_folder_leaf(leaf, case=case)

    # No real album identity left after tidying ("1", artist-name + junk) —
    # rebuild from the tracks' shared tags. A bigger leap, so it ships low
    # (default-unticked) and the operator reviews it.
    if not _folder_name_usable(tidied, clues):
        rebuilt = _rebuild_folder_name(folder, clues)
        if rebuilt and rebuilt != leaf:
            return FolderSuggestion(folder, leaf, rebuilt, ("rebuild_from_tags",), LOW)

    if tidied and tidied != leaf:
        return FolderSuggestion(folder, leaf, tidied, fired or (RULE_SEPARATORS,), HIGH)
    return None


def _folders_under(all_tracks: Sequence[TrackLike], folder: str) -> list[TrackLike]:
    prefix = f"{folder}/"
    return [t for t in all_tracks if t.path.startswith(prefix)]


def _all_folder_paths(tracks: Sequence[TrackLike]) -> set[str]:
    """Every distinct folder that appears as an ancestor of some track — the
    set a proposed rename must not collide with."""
    out: set[str] = set()
    for t in tracks:
        p = PurePosixPath(t.path).parent
        while str(p) not in (".", "/", ""):
            out.add(str(p))
            p = p.parent
    return out


def analyze_folders(
    scope_tracks: Sequence[TrackLike],
    all_tracks: Sequence[TrackLike],
    enabled: Collection[str] | None = None,
) -> list[FolderSuggestion]:
    """Propose folder renames for the folders the scoped tracks live in (plus
    the album-parent of any disc/part container among them). Each rename is
    leaf-only; the apply layer moves it with the index's `rename_folder` so
    every track path follows. Pure — no disk or DB access."""
    rules = frozenset(enabled) if enabled is not None else DEFAULT_RULES
    if RULE_FOLDERS not in rules:
        return []
    case = RULE_CASE in rules

    candidates: set[str] = set()
    for folder in _group_by_folder(scope_tracks):
        if not folder:
            continue  # never rename the music root
        candidates.add(folder)
        # A disc/part subfolder's album-parent is worth tidying too, even
        # though no track sits directly in it.
        if _disc_part_canonical(PurePosixPath(folder).name) is not None:
            parent = _parent(folder)
            if parent:
                candidates.add(parent)

    suggestions = [
        s
        for folder in candidates
        if (s := _plan_folder(folder, _folders_under(all_tracks, folder), case=case))
        is not None
    ]

    # Collision pass (deepest first, so a child is decided before its parent):
    # drop a target that already names another folder, or that two proposals
    # would both land on. Case-only renames pass (target "exists" = itself).
    existing_cf = {p.casefold() for p in _all_folder_paths(all_tracks)}
    taken: set[str] = set()
    kept: list[FolderSuggestion] = []
    for s in sorted(suggestions, key=lambda s: s.path.count("/"), reverse=True):
        parent = _parent(s.path)
        new_path = f"{parent}/{s.new}" if parent else s.new
        key = new_path.casefold()
        if key != s.path.casefold() and (key in existing_cf or key in taken):
            continue
        taken.add(key)
        kept.append(s)
    kept.sort(key=lambda s: s.path)
    return kept


def pending_lookups(
    plans: Sequence[TrackPlan], verdicts: Verdicts | None = None
) -> list[str]:
    """Distinct names whose artist-vs-album nature an online lookup could
    still settle: each plan's `wants_lookup` plus the values of remaining
    *low-confidence* artist/album suggestions. Deduped on the loose key
    (one lookup covers every track and spelling variant of the name) and
    ordered by first appearance. Tag-derived certainties are never
    second-guessed, so a fully-confident analysis asks for nothing."""
    seen: set[str] = set()
    out: list[str] = []

    def add(name: str) -> None:
        key = _loose(name.strip())
        if len(key) < 2 or key in seen or (verdicts is not None and key in verdicts):
            return
        seen.add(key)
        out.append(name.strip())

    for plan in plans:
        for wanted in plan.wants_lookup:
            add(wanted)
        for op in plan.ops:
            if (
                op.kind == "tag"
                and op.field in ("artist", "album")
                and op.confidence == LOW
            ):
                add(str(op.new or ""))
    return out


__all__ = [
    "ALL_RULES",
    "DEFAULT_RULES",
    "FolderSuggestion",
    "Suggestion",
    "TrackLike",
    "TrackPlan",
    "Verdicts",
    "analyze",
    "analyze_folders",
    "loose_key",
    "pending_lookups",
    "verdict_kind",
]
