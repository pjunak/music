"""Deterministic local playlist ranking and sequence planning.

The planner keeps operator tags, metadata profiles, and measured signal
profiles as separate evidence sources. It never writes playlists or tags;
the caller still reviews a draft through Authoring import.
"""
import re
import unicodedata
from collections.abc import Mapping, Sequence
from dataclasses import dataclass
from functools import cache
from typing import Literal

from app.assistant.engine import TrackAnalysisProfile, TrackLike, TrackSignalProfile
from app.assistant.schemas import (
    PlaylistAudioSignal,
    PlaylistCandidate,
    PlaylistIntent,
    PlaylistPlan,
    PlaylistSuggestionRequest,
    PlaylistSuggestionResponse,
)
from app.assistant.tag_vocabulary import default_tag_vocabulary_snapshot

_WORD_RE = re.compile(r"[a-z0-9]+")
_PLAYLIST_RETRIEVAL_ALIASES: dict[str, tuple[str, ...]] = {
    "stealth": ("burglary", "clandestine", "heist"),
}
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
        "festive",
        frozenset({"celebration", "dance", "dancing", "feast", "festive", "festival"}),
        0.72,
        0.82,
        0.12,
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
    match_terms: frozenset[str]


@dataclass(frozen=True)
class _RankedTrack:
    track: TrackLike
    score: float
    confidence: Literal["high", "medium", "low"]
    reasons: tuple[str, ...]
    manual_tags: tuple[str, ...]
    analysis_tags: tuple[str, ...]
    planning_energy: float
    signal_profile: TrackSignalProfile | None


def _tokens(value: str) -> frozenset[str]:
    normalized = unicodedata.normalize("NFKD", value.casefold())
    ascii_value = "".join(char for char in normalized if not unicodedata.combining(char))
    return frozenset(_WORD_RE.findall(ascii_value))


@cache
def _retrieval_vocabulary_phrases() -> tuple[
    tuple[str, tuple[frozenset[str], ...]], ...
]:
    vocabulary_phrases = tuple(
        (
            entry.name,
            tuple(
                phrase_tokens
                for phrase in (entry.name, *entry.aliases, *entry.context_cues)
                if (phrase_tokens := _tokens(phrase))
            ),
        )
        for entry in default_tag_vocabulary_snapshot().entries
    )
    playlist_aliases = tuple(
        (
            name,
            tuple(_tokens(alias) for alias in aliases),
        )
        for name, aliases in _PLAYLIST_RETRIEVAL_ALIASES.items()
    )
    return (*vocabulary_phrases, *playlist_aliases)


def expanded_retrieval_prompt(prompt: str) -> str:
    """Add canonical vocabulary terms for matched aliases and context cues."""

    prompt_tokens = _tokens(prompt)
    expansions = [
        name
        for name, phrases in _retrieval_vocabulary_phrases()
        if not _tokens(name) <= prompt_tokens
        and any(phrase <= prompt_tokens for phrase in phrases)
    ]
    return f"{prompt} {' '.join(expansions)}" if expansions else prompt


def _mean(values: list[float], default: float = 0.5) -> float:
    return sum(values) / len(values) if values else default


def _clamp(value: float) -> float:
    return max(0.0, min(1.0, value))


_PROFILE_WEIGHTS: dict[Literal["high", "medium", "low"], float] = {
    "high": 0.8,
    "medium": 0.55,
    "low": 0.3,
}


def _blend_axis(
    metadata_value: float,
    metadata_confidence: Literal["high", "medium", "low"],
    signal_value: float,
    signal_confidence: Literal["high", "medium", "low"],
    *,
    signal_multiplier: float,
) -> float:
    metadata_weight = _PROFILE_WEIGHTS[metadata_confidence]
    signal_weight = _PROFILE_WEIGHTS[signal_confidence] * signal_multiplier
    return _clamp(
        (metadata_value * metadata_weight + signal_value * signal_weight)
        / (metadata_weight + signal_weight)
    )


def _combined_axes(
    metadata: TrackAnalysisProfile,
    signal: TrackSignalProfile | None,
) -> tuple[float, float, float]:
    if signal is None:
        return metadata.energy, metadata.brightness, metadata.tension
    return (
        _blend_axis(
            metadata.energy,
            metadata.confidence,
            signal.energy,
            signal.confidence,
            signal_multiplier=1.0,
        ),
        _blend_axis(
            metadata.brightness,
            metadata.confidence,
            signal.brightness,
            signal.confidence,
            signal_multiplier=0.85,
        ),
        _blend_axis(
            metadata.tension,
            metadata.confidence,
            signal.tension,
            signal.confidence,
            signal_multiplier=0.6,
        ),
    )


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


def _track_field_tokens(
    track: TrackLike,
    manual_tags: Sequence[str] = (),
) -> dict[str, frozenset[str]]:
    canonical_title = track.display_title.strip() or track.title
    return {
        "title": _tokens(canonical_title),
        "genre": _tokens(track.genre),
        "origin": _tokens(track.origin),
        "album": _tokens(track.album),
        "artist": _tokens(track.artist),
        "path": _tokens(track.path),
        "manual_tags": frozenset(
            token for tag in manual_tags for token in _tokens(tag)
        ),
    }


def _track_axes(
    track: TrackLike, field_tokens: dict[str, frozenset[str]]
) -> tuple[float, float, float, tuple[str, ...]]:
    mood_tokens = frozenset().union(
        field_tokens["title"],
        field_tokens["genre"],
        field_tokens["album"],
    )
    track_moods = [profile for profile in _MOODS if mood_tokens & profile.aliases]

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


def _metadata_track_profile(
    track: TrackLike,
    field_tokens: dict[str, frozenset[str]],
) -> TrackAnalysisProfile:
    energy, brightness, tension, moods = _track_axes(track, field_tokens)
    evidence: list[str] = []
    if moods:
        evidence.append(f"Mood metadata: {', '.join(moods[:3])}")
    if track.bpm is not None:
        evidence.append(f"Tempo metadata: {track.bpm} BPM")
    if track.genre:
        evidence.append(f"Genre metadata: {track.genre}")
    confidence: Literal["high", "medium", "low"] = (
        "high" if len(evidence) >= 3 else "medium" if evidence else "low"
    )
    if not evidence:
        evidence.append("No explicit mood, genre, or tempo metadata")
    return TrackAnalysisProfile(
        energy=energy,
        brightness=brightness,
        tension=tension,
        moods=moods,
        evidence=tuple(evidence),
        confidence=confidence,
    )


def analyze_track_metadata(track: TrackLike) -> TrackAnalysisProfile:
    """Build the versioned local profile persisted by library analysis."""

    return _metadata_track_profile(track, _track_field_tokens(track))


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
        "manual_tags": 2.0,
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
    # Keep the original metadata scale (1.4) while allowing an explicit
    # operator tag to saturate the score instead of diluting existing matches.
    return _clamp(total / (len(terms) * 1.4)), tuple(matched)


def _rank_track(
    track: TrackLike,
    intent: _Intent,
    profile: TrackAnalysisProfile | None = None,
    manual_tags: Sequence[str] = (),
    signal_profile: TrackSignalProfile | None = None,
) -> _RankedTrack:
    metadata_field_tokens = _track_field_tokens(track)
    field_tokens = _track_field_tokens(track, manual_tags)
    profile = profile or _metadata_track_profile(track, metadata_field_tokens)
    energy, brightness, tension = _combined_axes(profile, signal_profile)
    track_moods = profile.moods
    semantic_score, matched_terms = _semantic_match(intent.tokens, field_tokens)
    manual_tokens = field_tokens["manual_tags"]
    manual_matches = tuple(sorted(intent.match_terms & manual_tokens))
    manual_moods = {
        mood.name for mood in _MOODS if mood.aliases & manual_tokens
    }
    manual_mood_matches = tuple(
        sorted(set(intent.public.matched_moods) & manual_moods)
    )

    weighted_scores: list[tuple[float, float]] = []
    manual_signal_score: float | None = None
    if manual_matches or manual_mood_matches:
        exact_score = (
            len(manual_matches) / len(intent.match_terms)
            if intent.match_terms
            else 0.0
        )
        mood_tag_score = (
            len(manual_mood_matches) / len(intent.public.matched_moods)
            if intent.public.matched_moods
            else 0.0
        )
        manual_signal_score = max(exact_score, mood_tag_score)
        weighted_scores.append((manual_signal_score, 0.75))
    if intent.public.matched_moods:
        mood_score = _mean(
            [
                1.0 - abs(intent.public.energy - energy),
                1.0 - abs(intent.public.brightness - brightness),
                1.0 - abs(intent.public.tension - tension),
            ]
        )
        weighted_scores.append((mood_score, 0.55 if weighted_scores else 0.68))
    if intent.tokens:
        weighted_scores.append((semantic_score, 0.3 if weighted_scores else 1.0))
    if not weighted_scores:
        weighted_scores.append((0.5, 1.0))

    score = sum(value * weight for value, weight in weighted_scores) / sum(
        weight for _, weight in weighted_scores
    )
    evidence_count = (
        len(matched_terms)
        + len(track_moods)
        + len(manual_matches)
        + len(manual_mood_matches)
    )
    if track.bpm is not None and intent.public.matched_moods:
        evidence_count += 1
    if track.genre:
        evidence_count += 1
    if signal_profile is not None:
        evidence_count += 2 if signal_profile.confidence == "high" else 1
    reliability = min(1.0, evidence_count / 3)
    score = _clamp(score * (0.88 + 0.12 * reliability))
    if manual_signal_score is not None:
        # Exact human classification is authoritative context. A partial tag
        # match helps without drowning out the remaining requested terms.
        score = max(score, 0.65 + 0.35 * manual_signal_score)

    reasons: list[str] = []
    if manual_matches or manual_mood_matches:
        matched_manual = tuple(
            tag
            for tag in manual_tags
            if _tokens(tag) & intent.match_terms
            or any(
                mood.name in manual_mood_matches and mood.aliases & _tokens(tag)
                for mood in _MOODS
            )
        )
        reasons.append(f"Your tags: {', '.join(matched_manual[:3])}")
    if matched_terms:
        reasons.append(f"Metadata matches: {', '.join(matched_terms[:3])}")
    if track_moods:
        reasons.append(f"Mood metadata: {', '.join(track_moods[:2])}")
    effective_bpm = (
        float(track.bpm)
        if track.bpm is not None
        else signal_profile.tempo_bpm
        if signal_profile is not None
        else None
    )
    if effective_bpm is not None and intent.public.matched_moods:
        pace = "calmer" if intent.public.energy < 0.4 else "higher-energy"
        if abs(intent.public.energy - energy) <= 0.25:
            source = "Measured tempo" if track.bpm is None else "Tempo"
            reasons.append(
                f"{source}: {effective_bpm:.0f} BPM supports the requested {pace} pace"
            )
        else:
            source = "Measured tempo" if track.bpm is None else "Tempo evidence"
            reasons.append(f"{source}: {effective_bpm:.0f} BPM")
    if signal_profile is not None and len(reasons) < 4:
        if abs(intent.public.energy - signal_profile.energy) <= 0.25:
            reasons.append("Measured audio energy supports the requested flow")
        else:
            reasons.append(f"Measured audio energy: {signal_profile.energy:.0%}")
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
        reasons=tuple(reasons[:4]),
        manual_tags=tuple(manual_tags),
        analysis_tags=track_moods,
        planning_energy=energy,
        signal_profile=signal_profile,
    )


def _effective_bpm(
    track: TrackLike,
    signal_profile: TrackSignalProfile | None,
) -> float | None:
    if track.bpm is not None:
        return float(track.bpm)
    return signal_profile.tempo_bpm if signal_profile is not None else None


def _eligible(
    track: TrackLike,
    request: PlaylistSuggestionRequest,
    signal_profile: TrackSignalProfile | None,
) -> bool:
    if track.id in request.exclude_track_ids:
        return False
    effective_bpm = _effective_bpm(track, signal_profile)
    if effective_bpm is None:
        return request.include_unknown_bpm
    if request.min_bpm is not None and effective_bpm < request.min_bpm:
        return False
    return request.max_bpm is None or effective_bpm <= request.max_bpm


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


def _planning_duration(track: TrackLike) -> float:
    return track.length_s if track.length_s > 0 else 180.0


def _default_pool(
    ranked: Sequence[_RankedTrack],
    target_seconds: float,
) -> tuple[list[_RankedTrack], float]:
    if not ranked:
        return [], 0.0

    durations = [_planning_duration(item.track) for item in ranked]
    selected_indices: set[int] = set()
    selected_seconds = 0.0
    for index, duration in enumerate(durations):
        if abs(selected_seconds + duration - target_seconds) < abs(
            selected_seconds - target_seconds
        ):
            selected_indices.add(index)
            selected_seconds += duration

    if not selected_indices:
        closest = min(
            range(len(ranked)),
            key=lambda index: (abs(durations[index] - target_seconds), index),
        )
        selected_indices.add(closest)
        selected_seconds = durations[closest]

    # Improve a rank-respecting greedy result with bounded add/remove/swap moves.
    # Each move must strictly reduce duration error, so this always terminates.
    while True:
        current_error = abs(selected_seconds - target_seconds)
        best_error = current_error
        best_indices: set[int] | None = None
        best_seconds = selected_seconds

        for index, duration in enumerate(durations):
            if index in selected_indices:
                if len(selected_indices) > 1:
                    candidate_seconds = selected_seconds - duration
                    candidate_error = abs(candidate_seconds - target_seconds)
                    if candidate_error < best_error:
                        best_error = candidate_error
                        best_indices = selected_indices - {index}
                        best_seconds = candidate_seconds
                continue
            candidate_seconds = selected_seconds + duration
            candidate_error = abs(candidate_seconds - target_seconds)
            if candidate_error < best_error:
                best_error = candidate_error
                best_indices = selected_indices | {index}
                best_seconds = candidate_seconds

        for removed in selected_indices:
            for added, duration in enumerate(durations):
                if added in selected_indices:
                    continue
                candidate_seconds = (
                    selected_seconds - durations[removed] + duration
                )
                candidate_error = abs(candidate_seconds - target_seconds)
                if candidate_error < best_error:
                    best_error = candidate_error
                    best_indices = (selected_indices - {removed}) | {added}
                    best_seconds = candidate_seconds

        if best_indices is None:
            break
        selected_indices = best_indices
        selected_seconds = best_seconds

    return (
        [item for index, item in enumerate(ranked) if index in selected_indices],
        selected_seconds,
    )


def _arc_targets(items: Sequence[_RankedTrack]) -> list[float]:
    if not items:
        return []
    energies = [item.planning_energy for item in items]
    low = min(energies)
    high = max(energies)
    if len(items) == 1 or high - low < 0.05:
        return [energies[0]] * len(items)
    targets: list[float] = []
    peak_position = 0.65
    for index in range(len(items)):
        position = index / (len(items) - 1)
        if position <= peak_position:
            targets.append(low + (high - low) * (position / peak_position))
        else:
            resolution = (position - peak_position) / (1.0 - peak_position)
            targets.append(high - (high - low) * 0.65 * resolution)
    return targets


def _sequence_default_pool(
    items: Sequence[_RankedTrack],
    energy_curve: Literal["steady", "rising", "falling", "arc"],
) -> list[_RankedTrack]:
    if energy_curve == "steady" or len(items) < 2:
        return list(items)
    rank = {item.track.id: index for index, item in enumerate(items)}
    if energy_curve == "rising":
        return sorted(
            items,
            key=lambda item: (
                item.planning_energy,
                rank[item.track.id],
            ),
        )
    if energy_curve == "falling":
        return sorted(
            items,
            key=lambda item: (
                -item.planning_energy,
                rank[item.track.id],
            ),
        )

    remaining = list(items)
    ordered: list[_RankedTrack] = []
    for target in _arc_targets(items):
        winner = min(
            remaining,
            key=lambda item: (
                abs(item.planning_energy - target),
                rank[item.track.id],
            ),
        )
        remaining.remove(winner)
        ordered.append(winner)
    return ordered


def suggest_local_playlist(
    tracks: Sequence[TrackLike],
    request: PlaylistSuggestionRequest,
    profiles: Mapping[int, TrackAnalysisProfile] | None = None,
    manual_tags: Mapping[int, Sequence[str]] | None = None,
    signal_profiles: Mapping[int, TrackSignalProfile] | None = None,
) -> PlaylistSuggestionResponse:
    public_intent = interpret_prompt(request.prompt)
    intent = _Intent(
        public=public_intent,
        tokens=frozenset(public_intent.search_terms),
        match_terms=_tokens(request.prompt) - _STOP_WORDS,
    )
    eligible = [
        track
        for track in tracks
        if _eligible(
            track,
            request,
            signal_profiles.get(track.id) if signal_profiles is not None else None,
        )
    ]
    ranked = [
        _rank_track(
            track,
            intent,
            profiles.get(track.id) if profiles is not None else None,
            manual_tags.get(track.id, ()) if manual_tags is not None else (),
            signal_profiles.get(track.id) if signal_profiles is not None else None,
        )
        for track in eligible
    ]
    diversified = _diversify(ranked, request.candidate_limit)

    target_seconds = request.target_minutes * 60.0
    default_pool, selected_seconds = _default_pool(diversified, target_seconds)
    selected_ids = {item.track.id for item in default_pool}
    planned_pool = _sequence_default_pool(default_pool, request.energy_curve)
    alternates = [item for item in diversified if item.track.id not in selected_ids]
    planned = [*planned_pool, *alternates]
    sequence_positions = {
        item.track.id: position
        for position, item in enumerate(planned_pool, start=1)
    }
    candidates: list[PlaylistCandidate] = []
    for item in planned:
        signal = item.signal_profile
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
                manual_tags=list(item.manual_tags),
                analysis_tags=list(item.analysis_tags),
                length_s=max(0.0, item.track.length_s),
                bpm=item.track.bpm,
                match_score=round(item.score, 4),
                confidence=item.confidence,
                reasons=list(item.reasons),
                default_selected=item.track.id in selected_ids,
                sequence_position=sequence_positions.get(item.track.id),
                planning_energy=round(item.planning_energy, 4),
                audio_signal=(
                    PlaylistAudioSignal(
                        analyzer_id=signal.analyzer_id,
                        energy=signal.energy,
                        brightness=signal.brightness,
                        tension=signal.tension,
                        tempo_bpm=signal.tempo_bpm,
                        confidence=signal.confidence,
                    )
                    if signal is not None
                    else None
                ),
            )
        )

    return PlaylistSuggestionResponse(
        engine="local-planner/v2",
        library_tracks=len(tracks),
        eligible_tracks=len(eligible),
        intent=public_intent,
        plan=PlaylistPlan(
            energy_curve=request.energy_curve,
            selected_tracks=len(default_pool),
            selected_duration_s=round(selected_seconds, 3),
            audio_profile_tracks=sum(
                item.signal_profile is not None for item in planned
            ),
        ),
        candidates=candidates,
    )


class LocalPlaylistPlanner:
    """Fast private planner retained when optional providers are added."""

    engine_id = "local-planner/v2"

    def suggest(
        self,
        tracks: Sequence[TrackLike],
        request: PlaylistSuggestionRequest,
        profiles: Mapping[int, TrackAnalysisProfile] | None = None,
        manual_tags: Mapping[int, Sequence[str]] | None = None,
        signal_profiles: Mapping[int, TrackSignalProfile] | None = None,
    ) -> PlaylistSuggestionResponse:
        return suggest_local_playlist(
            tracks,
            request,
            profiles,
            manual_tags,
            signal_profiles,
        )


local_playlist_planner = LocalPlaylistPlanner()
