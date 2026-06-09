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
from collections.abc import Collection, Sequence
from dataclasses import dataclass, field
from pathlib import PurePosixPath
from typing import Protocol

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
    }
)
# Case normalisation is opinionated (ALL-CAPS stems may be deliberate), so
# it's the one rule that defaults off.
DEFAULT_RULES: frozenset[str] = ALL_RULES - {RULE_CASE}

HIGH = "high"
LOW = "low"


class TrackLike(Protocol):
    """The slice of the Track row the engine reads. Tests can pass any
    object with these attributes; the API passes ORM rows."""

    id: int
    path: str
    title: str
    artist: str
    album_artist: str
    album: str
    track_no: int | None
    disc_no: int | None


@dataclass
class Suggestion:
    track_id: int
    kind: str  # "rename" | "tag"
    field: str | None  # tag field for kind="tag"; None for renames
    old: str | int | None
    new: str | int | None
    rules: tuple[str, ...]
    confidence: str  # HIGH | LOW

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


@dataclass
class _FolderCtx:
    name: str  # leaf folder name ("" at the music root)
    numbers_corroborated: bool
    seg1: str | None  # canonical text of a folder-wide shared first segment
    seg2: str | None


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


def _build_ctx(folder: str, raw_stems: Sequence[str]) -> _FolderCtx:
    name = PurePosixPath(folder).name if folder else ""
    stems = [_normalize_separators(s) for s in raw_stems]

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

    return _FolderCtx(name=name, numbers_corroborated=pass1 or pass2, seg1=seg1, seg2=seg2)


# --- per-track analysis -------------------------------------------------------


@dataclass
class _Extraction:
    track_no: int | None = None
    disc_no: int | None = None
    number_conf: str = LOW
    artist: str | None = None
    artist_conf: str = LOW
    album: str | None = None


def _classify_segment(
    seg: str, position: int, n_parts: int, track: TrackLike, ctx: _FolderCtx
) -> tuple[str, str, tuple[str, str, str] | None] | None:
    """Decide whether a leading " - " segment is redundant metadata.

    Returns (strip_rule, confidence, tag_guess) where tag_guess is
    (field, value, confidence) or None — or None when the segment looks
    like part of the actual title and must be kept.
    """
    artist_empty = not track.artist
    album_emptyish = not track.album or _loose_eq(track.album, ctx.name)

    if _loose_eq(seg, track.artist) or _loose_eq(seg, track.album_artist):
        return (RULE_ARTIST, HIGH, None)
    if _loose_eq(seg, track.album):
        return (RULE_ALBUM, HIGH, None)
    if _loose_eq(seg, ctx.name):
        # Folder named after the segment. Whether the folder is an artist
        # or an album is ambiguous; artist is the more common layout, so
        # the guess (if tags are empty) goes there — graded low.
        guess = ("artist", seg, HIGH if ctx.seg1 and _loose_eq(seg, ctx.seg1) else LOW)
        return (RULE_ARTIST, HIGH, guess if artist_empty else None)
    if position == 0 and ctx.seg1 is not None and _loose_eq(seg, ctx.seg1):
        conf = HIGH if _loose_eq(seg, ctx.name) else LOW
        return (RULE_ARTIST, HIGH, ("artist", seg, conf) if artist_empty else None)
    if position == 1 and ctx.seg2 is not None and _loose_eq(seg, ctx.seg2):
        return (RULE_ALBUM, HIGH, ("album", seg, LOW) if album_emptyish else None)
    if (
        position == 0
        and ctx.seg1 is None
        and n_parts == 2
        and artist_empty
        and not seg.strip().isdigit()  # "01 - Foo" with number-strip off isn't "artist 01"
    ):
        # Lone "A - B" file with no tags and no siblings to corroborate.
        # Most likely "Artist - Title", but it's a guess — ships as low.
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
        candidate, t_fired = _transform_stem(
            track.title, track, ctx, ALL_RULES if RULE_CASE in enabled else DEFAULT_RULES, t_extraction
        )
        if t_fired and candidate and candidate != track.title:
            plan.ops.append(
                Suggestion(
                    track_id=track.id,
                    kind="tag",
                    field="title",
                    old=track.title,
                    new=candidate,
                    rules=(RULE_TAG_TITLE,),
                    confidence=_worst(t_fired.values()),
                )
            )

    if RULE_TAG_ARTIST in enabled and not track.artist and extraction.artist:
        plan.ops.append(
            Suggestion(
                track_id=track.id,
                kind="tag",
                field="artist",
                old="",
                new=extraction.artist,
                rules=(RULE_TAG_ARTIST,),
                confidence=extraction.artist_conf,
            )
        )

    if (
        RULE_TAG_ALBUM in enabled
        and extraction.album
        and (not track.album or _loose_eq(track.album, ctx.name))
        and extraction.album != track.album
    ):
        plan.ops.append(
            Suggestion(
                track_id=track.id,
                kind="tag",
                field="album",
                old=track.album,
                new=extraction.album,
                rules=(RULE_TAG_ALBUM,),
                confidence=LOW,
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
        if track.disc_no is None and extraction.disc_no is not None:
            plan.ops.append(
                Suggestion(
                    track_id=track.id,
                    kind="tag",
                    field="disc_no",
                    old=None,
                    new=extraction.disc_no,
                    rules=(RULE_TAG_NUMBER,),
                    confidence=extraction.number_conf,
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


def analyze(
    scope_tracks: Sequence[TrackLike],
    all_tracks: Sequence[TrackLike],
    enabled: Collection[str] | None = None,
) -> list[TrackPlan]:
    """Build cleanup plans for `scope_tracks`. `all_tracks` supplies folder
    context (sibling corroboration) — pass every indexed track; tracks
    outside the scope inform the heuristics but get no suggestions."""
    rules = frozenset(enabled) if enabled is not None else DEFAULT_RULES

    by_folder_all: dict[str, list[TrackLike]] = {}
    for t in all_tracks:
        by_folder_all.setdefault(_parent(t.path), []).append(t)
    by_folder_scope: dict[str, list[TrackLike]] = {}
    for t in scope_tracks:
        by_folder_scope.setdefault(_parent(t.path), []).append(t)

    plans: list[TrackPlan] = []
    for folder, group in sorted(by_folder_scope.items()):
        # Corroborate within the scoped subset when it's big enough to mean
        # something (an album selected inside a junk-drawer folder), else
        # over the whole folder.
        ctx_tracks = group if len(group) >= 2 else by_folder_all.get(folder, group)
        ctx = _build_ctx(folder, [PurePosixPath(t.path).stem for t in ctx_tracks])

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

        plans.extend(p for p in folder_plans if p.ops or p.notes)

    return plans


__all__ = [
    "ALL_RULES",
    "DEFAULT_RULES",
    "Suggestion",
    "TrackLike",
    "TrackPlan",
    "analyze",
]
