from __future__ import annotations

from dataclasses import dataclass

from fastapi.testclient import TestClient

from app.assistant.local import interpret_prompt, suggest_local_playlist
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

    assert blank.status_code == 422
    assert reversed_range.status_code == 422


def test_assistant_endpoint_returns_current_library_tracks(auth_client: TestClient) -> None:
    response = auth_client.post(
        "/api/assistant/playlists/suggest",
        json={"prompt": "quiet ambient", "target_minutes": 15, "candidate_limit": 10},
    )

    assert response.status_code == 200, response.text
    payload = response.json()
    assert payload["engine"] == "local-metadata/v1"
    assert payload["library_tracks"] >= 1
    assert payload["eligible_tracks"] >= 1
    assert payload["candidates"]
    assert any(
        candidate["path"] == "Demo/test-song.wav"
        for candidate in payload["candidates"]
    )
