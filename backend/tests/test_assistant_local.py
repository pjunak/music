from dataclasses import dataclass
from typing import Literal

from fastapi.testclient import TestClient

from app.assistant.engine import TrackAnalysisProfile
from app.assistant.local import (
    analyze_track_metadata,
    expanded_retrieval_prompt,
    interpret_prompt,
    suggest_local_playlist,
)
from app.assistant.schemas import PlaylistSuggestionRequest


@dataclass
class StubTrack:
    id: int
    path: str
    title: str
    artist: str = ""
    album: str = ""
    origin: str = ""
    genre: str = ""
    display_title: str = ""
    length_s: float = 180
    bpm: int | None = None


@dataclass(frozen=True)
class StubSignalProfile:
    analyzer_id: str = "local-audio/v1"
    energy: float = 0.5
    brightness: float = 0.5
    tension: float = 0.5
    tempo_bpm: float | None = None
    confidence: Literal["high", "medium", "low"] = "high"


def request(prompt: str, **overrides: object) -> PlaylistSuggestionRequest:
    return PlaylistSuggestionRequest.model_validate(
        {"prompt": prompt, "candidate_limit": 10, **overrides}
    )


def test_interpret_prompt_combines_known_moods_and_keeps_context_terms() -> None:
    intent = interpret_prompt("dark rainy investigation with no vocals")

    assert intent.matched_moods == ["tense", "dark"]
    assert intent.search_terms == ["no", "rainy", "vocals"]
    assert intent.tension > 0.8
    assert intent.brightness < 0.3


def test_vocabulary_cues_expand_retrieval_without_rewriting_public_intent() -> None:
    expanded = expanded_retrieval_prompt("clandestine burglary under moonlight")

    assert expanded.startswith("clandestine burglary under moonlight")
    assert "stealth" in expanded.split()


def test_canonical_title_and_semantic_fields_bound_metadata_moods() -> None:
    track = StubTrack(
        1,
        "Battle/Dark Combat.flac",
        "Intense Battle",
        display_title="Neutral Ledger",
        artist="Dark Combat Ensemble",
        genre="classical",
    )

    profile = analyze_track_metadata(track)

    assert "combat" not in profile.moods
    assert "dark" not in profile.moods


def test_combat_and_calm_requests_rank_different_tracks() -> None:
    tracks = [
        StubTrack(1, "Ambient/Still Lake.flac", "Still Lake", genre="ambient", bpm=68),
        StubTrack(2, "Battle/Iron Charge.flac", "Iron Charge", genre="metal", bpm=158),
        StubTrack(3, "Town/Market Day.flac", "Market Day", genre="folk", bpm=104),
    ]

    combat = suggest_local_playlist(tracks, request("intense combat"))
    calm = suggest_local_playlist(tracks, request("quiet ambient rest"))

    assert combat.candidates[0].track_id == 2
    assert calm.candidates[0].track_id == 1
    assert combat.candidates[0].reasons
    assert calm.candidates[0].confidence in {"medium", "high"}


def test_filters_exclusions_and_default_duration_selection() -> None:
    tracks = [
        StubTrack(1, "one.flac", "One", length_s=180, bpm=80),
        StubTrack(2, "two.flac", "Two", length_s=180, bpm=100),
        StubTrack(3, "three.flac", "Three", length_s=180, bpm=None),
        StubTrack(4, "four.flac", "Four", length_s=180, bpm=140),
    ]

    result = suggest_local_playlist(
        tracks,
        request(
            "travel",
            target_minutes=5,
            min_bpm=90,
            max_bpm=120,
            include_unknown_bpm=False,
            exclude_track_ids=[1],
        ),
    )

    assert [candidate.track_id for candidate in result.candidates] == [2]
    assert result.candidates[0].default_selected is True
    assert result.eligible_tracks == 1


def test_default_selection_chooses_a_closer_duration_without_losing_rank_order() -> None:
    tracks = [
        StubTrack(1, "one.flac", "One", length_s=400),
        StubTrack(2, "two.flac", "Two", length_s=300),
        StubTrack(3, "three.flac", "Three", length_s=120),
    ]

    result = suggest_local_playlist(
        tracks,
        request("neutral scene", target_minutes=5),
    )

    selected = [item.track_id for item in result.candidates if item.default_selected]
    assert selected == [2]
    assert result.plan.selected_duration_s == 300


def test_current_analysis_profiles_feed_the_local_ranker() -> None:
    tracks = [
        StubTrack(1, "neutral-one.flac", "Neutral One"),
        StubTrack(2, "neutral-two.flac", "Neutral Two"),
    ]
    profiles = {
        1: TrackAnalysisProfile(0.2, 0.6, 0.1, ("calm",), ("cached",), "high"),
        2: TrackAnalysisProfile(0.9, 0.48, 0.76, ("combat",), ("cached",), "high"),
    }

    result = suggest_local_playlist(tracks, request("intense combat"), profiles)

    assert result.candidates[0].track_id == 2


def test_manual_tags_are_ranked_separately_from_analysis_tags() -> None:
    tracks = [
        StubTrack(1, "neutral-one.flac", "Neutral One"),
        StubTrack(2, "neutral-two.flac", "Neutral Two"),
    ]
    profiles = {
        1: TrackAnalysisProfile(0.5, 0.5, 0.5, ("dark",), ("cached",), "low"),
        2: TrackAnalysisProfile(0.5, 0.5, 0.5, ("dark",), ("cached",), "low"),
    }
    manual_tags = {2: ("medieval", "tavern", "dancing")}

    result = suggest_local_playlist(
        tracks,
        request("medieval tavern dancing"),
        profiles,
        manual_tags,
    )

    assert result.candidates[0].track_id == 2
    assert result.candidates[0].manual_tags == ["medieval", "tavern", "dancing"]
    assert result.candidates[0].analysis_tags == ["dark"]
    assert result.candidates[0].reasons[0].startswith("Your tags:")


def test_current_audio_profiles_influence_axes_without_becoming_tags() -> None:
    tracks = [
        StubTrack(1, "neutral-one.flac", "Neutral One"),
        StubTrack(2, "neutral-two.flac", "Neutral Two"),
    ]
    metadata_profiles = {
        track.id: TrackAnalysisProfile(0.5, 0.5, 0.5, (), ("neutral",), "low")
        for track in tracks
    }
    signals = {
        1: StubSignalProfile(energy=0.15, brightness=0.2, tension=0.2),
        2: StubSignalProfile(energy=0.95, brightness=0.5, tension=0.8),
    }

    result = suggest_local_playlist(
        tracks,
        request("intense combat"),
        metadata_profiles,
        signal_profiles=signals,
    )

    assert result.engine == "local-planner/v2"
    assert result.candidates[0].track_id == 2
    assert result.candidates[0].analysis_tags == []
    assert result.candidates[0].audio_signal is not None
    assert result.candidates[0].audio_signal.analyzer_id == "local-audio/v1"
    assert any("Measured audio energy" in reason for reason in result.candidates[0].reasons)


def test_audio_tempo_fills_missing_metadata_for_filters() -> None:
    tracks = [
        StubTrack(1, "one.flac", "One", bpm=None),
        StubTrack(2, "two.flac", "Two", bpm=None),
        StubTrack(3, "three.flac", "Three", bpm=None),
    ]
    signals = {
        1: StubSignalProfile(tempo_bpm=100.0),
        2: StubSignalProfile(tempo_bpm=140.0),
    }

    result = suggest_local_playlist(
        tracks,
        request(
            "travel",
            min_bpm=90,
            max_bpm=120,
            include_unknown_bpm=False,
        ),
        signal_profiles=signals,
    )

    assert [candidate.track_id for candidate in result.candidates] == [1]
    assert result.candidates[0].bpm is None
    assert result.candidates[0].audio_signal is not None
    assert result.candidates[0].audio_signal.tempo_bpm == 100.0


def test_planner_orders_default_selection_by_requested_energy_curve() -> None:
    tracks = [
        StubTrack(1, "one.flac", "One", length_s=180),
        StubTrack(2, "two.flac", "Two", length_s=180),
        StubTrack(3, "three.flac", "Three", length_s=180),
    ]
    manual_tags = {track.id: ("combat",) for track in tracks}
    signals = {
        1: StubSignalProfile(energy=0.2),
        2: StubSignalProfile(energy=0.5),
        3: StubSignalProfile(energy=0.9),
    }

    rising = suggest_local_playlist(
        tracks,
        request("combat", target_minutes=15, energy_curve="rising"),
        manual_tags=manual_tags,
        signal_profiles=signals,
    )
    falling = suggest_local_playlist(
        tracks,
        request("combat", target_minutes=15, energy_curve="falling"),
        manual_tags=manual_tags,
        signal_profiles=signals,
    )
    arc = suggest_local_playlist(
        tracks,
        request("combat", target_minutes=15, energy_curve="arc"),
        manual_tags=manual_tags,
        signal_profiles=signals,
    )

    assert [candidate.track_id for candidate in rising.candidates] == [1, 2, 3]
    assert [candidate.track_id for candidate in falling.candidates] == [3, 2, 1]
    assert [candidate.track_id for candidate in arc.candidates] == [1, 3, 2]
    assert rising.plan.energy_curve == "rising"
    assert rising.plan.selected_tracks == 3
    assert rising.plan.selected_duration_s == 540
    assert rising.plan.audio_profile_tracks == 3
    assert [candidate.sequence_position for candidate in rising.candidates] == [1, 2, 3]


def test_assistant_endpoint_requires_auth(client: TestClient) -> None:
    response = client.post(
        "/api/assistant/playlists/suggest",
        json={"prompt": "calm exploration"},
    )
    assert response.status_code == 401


def test_assistant_endpoint_rejects_invalid_local_filters(auth_client: TestClient) -> None:
    blank = auth_client.post(
        "/api/assistant/playlists/suggest",
        json={"prompt": "   "},
    )
    reversed_range = auth_client.post(
        "/api/assistant/playlists/suggest",
        json={"prompt": "calm", "min_bpm": 120, "max_bpm": 80},
    )
    unknown_curve = auth_client.post(
        "/api/assistant/playlists/suggest",
        json={"prompt": "calm", "energy_curve": "random"},
    )

    assert blank.status_code == 422
    assert reversed_range.status_code == 422
    assert unknown_curve.status_code == 422


def test_assistant_endpoint_returns_current_library_tracks(auth_client: TestClient) -> None:
    response = auth_client.post(
        "/api/assistant/playlists/suggest",
        json={"prompt": "quiet ambient", "target_minutes": 15, "candidate_limit": 10},
    )

    assert response.status_code == 200, response.text
    payload = response.json()
    assert payload["engine"] == "local-planner/v2"
    assert payload["library_tracks"] >= 1
    assert payload["eligible_tracks"] >= 1
    assert payload["candidates"]
    assert any(
        candidate["path"] == "Demo/test-song.wav"
        for candidate in payload["candidates"]
    )


def test_assistant_endpoint_uses_current_audio_profiles(
    auth_client: TestClient,
    seeded_track_id: int,
) -> None:
    from sqlalchemy import delete

    from app.assistant.audio_analysis import (
        LOCAL_AUDIO_ANALYZER_ID,
        audio_source_signature,
    )
    from app.core.db import SessionLocal
    from app.models.track import Track
    from app.models.track_analysis import TrackAnalysis

    with SessionLocal() as db:
        track = db.get(Track, seeded_track_id)
        assert track is not None
        db.add(
            TrackAnalysis(
                track_id=track.id,
                analyzer_id=LOCAL_AUDIO_ANALYZER_ID,
                source_signature=audio_source_signature(track),
                job_id="c" * 32,
                energy=0.82,
                brightness=0.61,
                tension=0.74,
                moods_json="[]",
                evidence_json='["Measured signal evidence"]',
                metrics_json=(
                    '{"schema":"local-audio/v1","rms_dbfs":-12.0,'
                    '"tempo_bpm":128.0}'
                ),
                confidence="high",
            )
        )
        db.commit()

    try:
        response = auth_client.post(
            "/api/assistant/playlists/suggest",
            json={
                "prompt": "intense combat",
                "energy_curve": "rising",
                "candidate_limit": 10,
            },
        )

        assert response.status_code == 200, response.text
        payload = response.json()
        candidate = next(
            item for item in payload["candidates"] if item["track_id"] == seeded_track_id
        )
        assert payload["plan"]["energy_curve"] == "rising"
        assert payload["plan"]["audio_profile_tracks"] == 1
        assert candidate["analysis_tags"] == []
        assert candidate["audio_signal"] == {
            "analyzer_id": LOCAL_AUDIO_ANALYZER_ID,
            "energy": 0.82,
            "brightness": 0.61,
            "tension": 0.74,
            "tempo_bpm": 128.0,
            "confidence": "high",
        }
    finally:
        with SessionLocal() as db:
            db.execute(
                delete(TrackAnalysis).where(
                    TrackAnalysis.track_id == seeded_track_id,
                    TrackAnalysis.analyzer_id == LOCAL_AUDIO_ANALYZER_ID,
                )
            )
            db.commit()
