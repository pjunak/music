"""Deterministic, metadata-only playlist suggestions.

This baseline intentionally does not claim to analyse audio.  It combines
words already present in the library index with BPM-derived energy, then
diversifies the ranked list.  Later audio classifiers and embedding models
can implement the same public contracts while keeping this fast, private
fallback available.
"""
from __future__ import annotations

import re
import unicodedata
from collections.abc import Sequence
from dataclasses import dataclass
from typing import Literal

from app.assistant.engine import TrackLike
from app.assistant.schemas import (
    PlaylistCandidate,
    PlaylistIntent,
    PlaylistSuggestionRequest,
    PlaylistSuggestionResponse,
)

_WORD_RE = re.compile(r"[a-z0-9]+")
_STOP_WORDS = frozenset(
    {
        "a",
        "an",
        "and",
        "for",
        "from",
        "in",
        "into",
        "music",
        "of",
        "on",
        "playlist",
        "song",
        "songs",
        "the",
        "to",
        "track",
        "tracks",
        "with",
    }
)


@dataclass(frozen=True)
class MoodProfile:
    name: str
    aliases: frozenset[str]
    energy: float
    brightness: float
    tension: float


_MOODS: tuple[MoodProfile, ...] = (
    MoodProfile(
        "calm",
        frozenset({"ambient", "calm", "gentle", "peaceful", "quiet", "relaxed", "rest"}),
        0.20,
        0.60,
        0.15,
    ),
    MoodProfile(
        "tense",
        frozenset({"investigation", "mystery", "ominous", "suspense", "tense", "tension"}),
        0.50,
        0.30,
        0.82,
    ),
    MoodProfile(
        "combat",
        frozenset({"action", "battle", "boss", "combat", "epic", "fight", "intense"}),
        0.90,
        0.48,
        0.76,
    ),
    MoodProfile(
        "dark",
        frozenset({"dark", "dread", "haunting", "horror", "scary", "sinister"}),
        0.45,
        0.12,
        0.90,
    ),
    MoodProfile(
        "bright",
        frozenset({"bright", "happy", "hopeful", "joyful", "triumphant", "uplifting"}),
        0.62,
        0.88,
        0.18,
    ),
    MoodProfile(
        "melancholy",
        frozenset({"grief", "melancholy", "sad", "somber", "sorrow", "tragic"}),
        0.28,
        0.14,
        0.35,
    ),
    MoodProfile(
        "tavern",
        frozenset({"acoustic", "folk", "inn", "medieval", "tavern"}),
        0.44,
        0.68,
        0.18,
    ),
    MoodProfile(
        "exploration",
        frozenset({"adventure", "exploration", "journey", "travel", "wilderness"}),
        0.46,
        0.58,
        0.32,
    ),
)

_ALL_MOOD_ALIASES = frozenset(alias for profile in _MOODS for alias in profile.aliases)
_GENRE_ENERGY: dict[str, float] = {
    "ambient": 0.15,
    "classical": 0.34,
    "acoustic": 0.30,
    "folk": 0.42,
    "jazz": 0.45,
    "soundtrack": 0.55,
    "cinematic": 0.58,
    "electronic": 0.68,
    "rock": 0.72,
    "dance": 0.82,
    "metal": 0.90,
}


@dataclass(frozen=True)
class _Intent:
    public: PlaylistIntent
    tokens: frozenset[str]


@dataclass(frozen=True)
class _RankedTrack:
    track: TrackLike
    score: float
    confidence: Literal["high", "medium", "low"]
    reasons: tuple[str, ...]


def _tokens(value: str) -> frozenset[str]:
    normalized = unicodedata.normalize("NFKD", value.casefold())
    ascii_value = "".join(char for char in normalized if not unicodedata.combining(char))
    return frozenset(_WORD_RE.findall(ascii_value))


def _mean(values: list[float], default: float = 0.5) -> float:
    return sum(values) / len(values) if values else default


def _clamp(value: float) -> float:
    return max(0.0, min(1.0, value))


def interpret_prompt(prompt: str) -> PlaylistIntent:
    """Translate common mood words into a small, explainable local profile."""

    prompt_tokens = _tokens(prompt)
    matched = [profile for profile in _MOODS if prompt_tokens & profile.aliases]
    semantic_terms = sorted(
        token
        for token in prompt_tokens
        if token not in _STOP_WORDS and token not in _ALL_MOOD_ALIASES
    )
    if not semantic_terms and not matched:
        semantic_terms = sorted(prompt_tokens - _STOP_WORDS)
    return PlaylistIntent(
        matched_moods=[profile.name for profile in matched],
        search_terms=semantic_terms,
        energy=_mean([profile.energy for profile in matched]),
        brightness=_mean([profile.brightness for profile in matched]),
        tension=_mean([profile.tension for profile in matched]),
    )


def _track_field_tokens(track: TrackLike) -> dict[str, frozenset[str]]:
    return {
        "title": _tokens(f"{track.display_title} {track.title}"),
        "genre": _tokens(track.genre),
        "origin": _tokens(track.origin),
        "album": _tokens(track.album),
        "artist": _tokens(track.artist),
        "path": _tokens(track.path),
    }


def _track_axes(
    track: TrackLike, field_tokens: dict[str, frozenset[str]]
) -> tuple[float, float, float, tuple[str, ...]]:
    all_tokens = frozenset().union(*field_tokens.values())
    track_moods = [profile for profile in _MOODS if all_tokens & profile.aliases]

    energy_values = [profile.energy for profile in track_moods]
    if track.bpm is not None:
        energy_values.append(_clamp((track.bpm - 55) / 125))
    for token, prior in _GENRE_ENERGY.items():
        if token in field_tokens["genre"]:
            energy_values.append(prior)

    return (
        _mean(energy_values),
        _mean([profile.brightness for profile in track_moods]),
        _mean([profile.tension for profile in track_moods]),
        tuple(profile.name for profile in track_moods),
    )


def _semantic_match(
    terms: frozenset[str], field_tokens: dict[str, frozenset[str]]
) -> tuple[float, tuple[str, ...]]:
    if not terms:
        return 0.0, ()
    weights = {
        "title": 1.4,
        "genre": 1.4,
        "origin": 1.2,
        "album": 0.9,
        "artist": 0.6,
        "path": 0.5,
    }
    matched: list[str] = []
    total = 0.0
    for term in sorted(terms):
        best = max(
            (weight for field, weight in weights.items() if term in field_tokens[field]),
            default=0.0,
        )
        if best > 0:
            matched.append(term)
            total += best
    return _clamp(total / (len(terms) * max(weights.values()))), tuple(matched)


def _rank_track(track: TrackLike, intent: _Intent) -> _RankedTrack:
    field_tokens = _track_field_tokens(track)
    energy, brightness, tension, track_moods = _track_axes(track, field_tokens)
    semantic_score, matched_terms = _semantic_match(intent.tokens, field_tokens)

    weighted_scores: list[tuple[float, float]] = []
    if intent.public.matched_moods:
        mood_score = _mean(
            [
                1.0 - abs(intent.public.energy - energy),
                1.0 - abs(intent.public.brightness - brightness),
                1.0 - abs(intent.public.tension - tension),
            ]
        )
        weighted_scores.append((mood_score, 0.68))
    if intent.tokens:
        weighted_scores.append((semantic_score, 0.32 if weighted_scores else 1.0))
    if not weighted_scores:
        weighted_scores.append((0.5, 1.0))

    score = sum(value * weight for value, weight in weighted_scores) / sum(
        weight for _, weight in weighted_scores
    )
    evidence_count = len(matched_terms) + len(track_moods)
    if track.bpm is not None and intent.public.matched_moods:
        evidence_count += 1
    if track.genre:
        evidence_count += 1
    reliability = min(1.0, evidence_count / 3)
    score = _clamp(score * (0.88 + 0.12 * reliability))

    reasons: list[str] = []
    if matched_terms:
        reasons.append(f"Metadata matches: {', '.join(matched_terms[:3])}")
    if track_moods:
        reasons.append(f"Mood metadata: {', '.join(track_moods[:2])}")
    if track.bpm is not None and intent.public.matched_moods:
        pace = "calmer" if intent.public.energy < 0.4 else "higher-energy"
        if abs(intent.public.energy - energy) <= 0.25:
            reasons.append(f"{track.bpm} BPM supports the requested {pace} pace")
        else:
            reasons.append(f"Tempo evidence: {track.bpm} BPM")
    if track.genre and len(reasons) < 3:
        reasons.append(f"Genre metadata: {track.genre}")
    if not reasons:
        reasons.append("Limited mood metadata; this is a low-confidence local match")

    confidence: Literal["high", "medium", "low"] = (
        "high" if evidence_count >= 3 else "medium" if evidence_count else "low"
    )
    return _RankedTrack(
        track=track,
        score=score,
        confidence=confidence,
        reasons=tuple(reasons[:3]),
    )


def _eligible(track: TrackLike, request: PlaylistSuggestionRequest) -> bool:
    if track.id in request.exclude_track_ids:
        return False
    if track.bpm is None:
        return request.include_unknown_bpm
    if request.min_bpm is not None and track.bpm < request.min_bpm:
        return False
    return request.max_bpm is None or track.bpm <= request.max_bpm


def _diversify(ranked: list[_RankedTrack], limit: int) -> list[_RankedTrack]:
    remaining = list(ranked)
    selected: list[_RankedTrack] = []
    artist_counts: dict[str, int] = {}
    album_counts: dict[str, int] = {}
    origin_counts: dict[str, int] = {}

    def adjusted(candidate: _RankedTrack) -> tuple[float, float, int]:
        artist = candidate.track.artist.casefold().strip()
        album = candidate.track.album.casefold().strip()
        origin = candidate.track.origin.casefold().strip()
        penalty = 0.0
        if artist:
            penalty += min(0.18, 0.08 * artist_counts.get(artist, 0))
        if album:
            penalty += min(0.12, 0.05 * album_counts.get(album, 0))
        if origin:
            penalty += min(0.08, 0.04 * origin_counts.get(origin, 0))
        return (candidate.score - penalty, candidate.score, -candidate.track.id)

    while remaining and len(selected) < limit:
        winner = max(remaining, key=adjusted)
        remaining.remove(winner)
        selected.append(winner)
        for value, counts in (
            (winner.track.artist, artist_counts),
            (winner.track.album, album_counts),
            (winner.track.origin, origin_counts),
        ):
            key = value.casefold().strip()
            if key:
                counts[key] = counts.get(key, 0) + 1
    return selected


def suggest_local_playlist(
    tracks: Sequence[TrackLike], request: PlaylistSuggestionRequest
) -> PlaylistSuggestionResponse:
    public_intent = interpret_prompt(request.prompt)
    intent = _Intent(public=public_intent, tokens=frozenset(public_intent.search_terms))
    eligible = [track for track in tracks if _eligible(track, request)]
    ranked = [_rank_track(track, intent) for track in eligible]
    diversified = _diversify(ranked, request.candidate_limit)

    target_seconds = request.target_minutes * 60
    selected_seconds = 0.0
    candidates: list[PlaylistCandidate] = []
    for item in diversified:
        default_selected = selected_seconds < target_seconds
        if default_selected:
            selected_seconds += item.track.length_s if item.track.length_s > 0 else 180
        candidates.append(
            PlaylistCandidate(
                track_id=item.track.id,
                path=item.track.path,
                title=item.track.title,
                display_title=item.track.display_title,
                artist=item.track.artist,
                album=item.track.album,
                origin=item.track.origin,
                genre=item.track.genre,
                length_s=max(0.0, item.track.length_s),
                bpm=item.track.bpm,
                match_score=round(item.score, 4),
                confidence=item.confidence,
                reasons=list(item.reasons),
                default_selected=default_selected,
            )
        )

    return PlaylistSuggestionResponse(
        engine="local-metadata/v1",
        library_tracks=len(tracks),
        eligible_tracks=len(eligible),
        intent=public_intent,
        candidates=candidates,
    )


class LocalMetadataPlaylistEngine:
    """Fast private baseline retained even when optional providers are added."""

    engine_id = "local-metadata/v1"

    def suggest(
        self,
        tracks: Sequence[TrackLike],
        request: PlaylistSuggestionRequest,
    ) -> PlaylistSuggestionResponse:
        return suggest_local_playlist(tracks, request)


local_metadata_playlist_engine = LocalMetadataPlaylistEngine()
