from fastapi.testclient import TestClient


def test_search_requires_auth(client: TestClient) -> None:
    response = client.get("/api/library/search")
    assert response.status_code == 401


def test_search_returns_seeded_track(auth_client: TestClient) -> None:
    response = auth_client.get("/api/library/search")
    assert response.status_code == 200
    body = response.json()
    assert body["limit"] == 100
    assert body["offset"] == 0
    assert len(body["tracks"]) == 1

    track = body["tracks"][0]
    assert track["title"] == "Test Song"
    assert track["artist"] == "Test Artist"
    assert track["album"] == "Test Album"


def test_search_query_filter_excludes_non_matches(auth_client: TestClient) -> None:
    response = auth_client.get("/api/library/search", params={"q": "artist:nonexistent"})
    assert response.status_code == 200
    assert response.json()["tracks"] == []


def test_search_pagination(auth_client: TestClient) -> None:
    response = auth_client.get("/api/library/search", params={"limit": 1, "offset": 1})
    assert response.status_code == 200
    assert response.json()["tracks"] == []


def test_get_track_by_id(auth_client: TestClient) -> None:
    seeded_id = auth_client.get("/api/library/search").json()["tracks"][0]["beets_id"]
    response = auth_client.get(f"/api/library/tracks/{seeded_id}")
    assert response.status_code == 200
    assert response.json()["title"] == "Test Song"


def test_get_track_missing_returns_404(auth_client: TestClient) -> None:
    response = auth_client.get("/api/library/tracks/999999")
    assert response.status_code == 404


def test_stream_returns_audio_bytes_inline(auth_client: TestClient) -> None:
    seeded_id = auth_client.get("/api/library/search").json()["tracks"][0]["beets_id"]
    response = auth_client.get(f"/api/library/tracks/{seeded_id}/stream")
    assert response.status_code == 200
    assert len(response.content) == 1100
    # `inline` (or absent attachment) lets <audio> play instead of forcing download.
    assert "attachment" not in response.headers.get("content-disposition", "")


def test_stream_supports_range_requests(auth_client: TestClient) -> None:
    seeded_id = auth_client.get("/api/library/search").json()["tracks"][0]["beets_id"]
    response = auth_client.get(
        f"/api/library/tracks/{seeded_id}/stream",
        headers={"Range": "bytes=0-9"},
    )
    assert response.status_code == 206
    assert len(response.content) == 10
    assert "content-range" in {k.lower() for k in response.headers}


def test_stream_missing_track_returns_404(auth_client: TestClient) -> None:
    response = auth_client.get("/api/library/tracks/999999/stream")
    assert response.status_code == 404
