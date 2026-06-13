"""Playlist CRUD + tracks endpoints. Playlists are manual-only now;
smart playlists are deferred (see docs/FUTURE.md)."""
from fastapi.testclient import TestClient
from sqlalchemy import select
from sqlalchemy.orm import Session

# --- playlist CRUD ----------------------------------------------------------


def test_create_playlist(auth_client: TestClient) -> None:
    r = auth_client.post(
        "/api/playlists", json={"name": "Combat", "mode_id": "dnd"}
    )
    assert r.status_code == 201
    body = r.json()
    assert body["name"] == "Combat"
    assert body["mode_id"] == "dnd"


def test_create_validates_mode_id(auth_client: TestClient) -> None:
    r = auth_client.post(
        "/api/playlists", json={"name": "X", "mode_id": "nope"}
    )
    assert r.status_code == 400


def test_list_filters_strictly_by_mode(auth_client: TestClient) -> None:
    # Playlists are per-mode now — filtering by mode returns only that mode's,
    # with no global tier mixed in.
    auth_client.post(
        "/api/playlists",
        json={"name": "DnD-Combat", "mode_id": "dnd", "category": "combat"},
    )
    auth_client.post(
        "/api/playlists", json={"name": "Orphan-Combat", "category": "combat"}
    )

    listed = auth_client.get(
        "/api/playlists", params={"mode_id": "dnd", "category": "combat"}
    ).json()
    names = {p["name"] for p in listed}
    assert "DnD-Combat" in names
    assert "Orphan-Combat" not in names
    assert all(p["mode_id"] == "dnd" for p in listed)


def test_create_and_update_accept_category(auth_client: TestClient) -> None:
    r = auth_client.post(
        "/api/playlists", json={"name": "Cat", "category": "exploration"}
    )
    assert r.status_code == 201
    assert r.json()["category"] == "exploration"

    pid = r.json()["id"]
    r2 = auth_client.patch(f"/api/playlists/{pid}", json={"category": "wonder"})
    assert r2.status_code == 200
    assert r2.json()["category"] == "wonder"


def test_get_playlist_404(auth_client: TestClient) -> None:
    assert auth_client.get("/api/playlists/999999").status_code == 404


def test_update_playlist_renames_and_changes_mode(auth_client: TestClient) -> None:
    pid = auth_client.post("/api/playlists", json={"name": "old"}).json()["id"]
    r = auth_client.patch(
        f"/api/playlists/{pid}", json={"name": "new", "mode_id": "dnd"}
    )
    assert r.status_code == 200
    body = r.json()
    assert body["name"] == "new"
    assert body["mode_id"] == "dnd"


def test_update_validates_mode(auth_client: TestClient) -> None:
    pid = auth_client.post("/api/playlists", json={"name": "x"}).json()["id"]
    r = auth_client.patch(f"/api/playlists/{pid}", json={"mode_id": "nope"})
    assert r.status_code == 400


def test_delete_playlist(auth_client: TestClient) -> None:
    pid = auth_client.post("/api/playlists", json={"name": "doomed"}).json()["id"]
    assert auth_client.delete(f"/api/playlists/{pid}").status_code == 204
    assert auth_client.get(f"/api/playlists/{pid}").status_code == 404


# --- playlist tracks --------------------------------------------------------


def test_add_track_appends_then_returns_in_order(
    auth_client: TestClient, seeded_track_id: int, extra_seeded_track_ids: list[int]
) -> None:
    pid = auth_client.post("/api/playlists", json={"name": "ord"}).json()["id"]
    expected = [seeded_track_id, *extra_seeded_track_ids]
    for tid in expected:
        auth_client.post(f"/api/playlists/{pid}/tracks", json={"track_id": tid})

    rows = auth_client.get(f"/api/playlists/{pid}/tracks").json()
    assert [r["position"] for r in rows] == [0, 1, 2, 3]
    assert [r["track_id"] for r in rows] == expected


def test_add_track_at_position_shifts_others(
    auth_client: TestClient, extra_seeded_track_ids: list[int]
) -> None:
    pid = auth_client.post("/api/playlists", json={"name": "shift"}).json()["id"]
    a, b, c = extra_seeded_track_ids
    auth_client.post(f"/api/playlists/{pid}/tracks", json={"track_id": a})
    auth_client.post(f"/api/playlists/{pid}/tracks", json={"track_id": c})
    auth_client.post(
        f"/api/playlists/{pid}/tracks", json={"track_id": b, "position": 1}
    )
    rows = auth_client.get(f"/api/playlists/{pid}/tracks").json()
    assert [r["track_id"] for r in rows] == [a, b, c]
    assert [r["position"] for r in rows] == [0, 1, 2]


def test_add_track_validates_track_id(auth_client: TestClient) -> None:
    pid = auth_client.post("/api/playlists", json={"name": "x"}).json()["id"]
    r = auth_client.post(f"/api/playlists/{pid}/tracks", json={"track_id": 999999})
    assert r.status_code == 404


def test_add_track_position_out_of_range(
    auth_client: TestClient, seeded_track_id: int
) -> None:
    pid = auth_client.post("/api/playlists", json={"name": "x"}).json()["id"]
    r = auth_client.post(
        f"/api/playlists/{pid}/tracks",
        json={"track_id": seeded_track_id, "position": 5},
    )
    assert r.status_code == 400


def test_remove_track_shifts_down(
    auth_client: TestClient, extra_seeded_track_ids: list[int]
) -> None:
    pid = auth_client.post("/api/playlists", json={"name": "rm"}).json()["id"]
    a, b, c = extra_seeded_track_ids
    for tid in [a, b, c]:
        auth_client.post(f"/api/playlists/{pid}/tracks", json={"track_id": tid})

    assert auth_client.delete(f"/api/playlists/{pid}/tracks/0").status_code == 204
    rows = auth_client.get(f"/api/playlists/{pid}/tracks").json()
    assert [r["track_id"] for r in rows] == [b, c]


def test_remove_track_404_for_missing_position(auth_client: TestClient) -> None:
    pid = auth_client.post("/api/playlists", json={"name": "x"}).json()["id"]
    assert auth_client.delete(f"/api/playlists/{pid}/tracks/0").status_code == 404


def test_move_track_forward(
    auth_client: TestClient, extra_seeded_track_ids: list[int]
) -> None:
    pid = auth_client.post("/api/playlists", json={"name": "mv"}).json()["id"]
    a, b, c = extra_seeded_track_ids
    for tid in [a, b, c]:
        auth_client.post(f"/api/playlists/{pid}/tracks", json={"track_id": tid})

    assert auth_client.patch(
        f"/api/playlists/{pid}/tracks/0", json={"to_position": 2}
    ).status_code == 204
    rows = auth_client.get(f"/api/playlists/{pid}/tracks").json()
    assert [r["track_id"] for r in rows] == [b, c, a]


def test_move_track_backward(
    auth_client: TestClient, extra_seeded_track_ids: list[int]
) -> None:
    pid = auth_client.post("/api/playlists", json={"name": "mvb"}).json()["id"]
    a, b, c = extra_seeded_track_ids
    for tid in [a, b, c]:
        auth_client.post(f"/api/playlists/{pid}/tracks", json={"track_id": tid})

    auth_client.patch(f"/api/playlists/{pid}/tracks/2", json={"to_position": 0})
    rows = auth_client.get(f"/api/playlists/{pid}/tracks").json()
    assert [r["track_id"] for r in rows] == [c, a, b]


def test_get_tracks_includes_track_summary(
    auth_client: TestClient, seeded_track_id: int
) -> None:
    pid = auth_client.post("/api/playlists", json={"name": "withmeta"}).json()["id"]
    auth_client.post(
        f"/api/playlists/{pid}/tracks", json={"track_id": seeded_track_id}
    )
    rows = auth_client.get(f"/api/playlists/{pid}/tracks").json()
    assert rows[0]["track"] is not None
    assert rows[0]["track"]["path"] == "Demo/test-song.wav"


def test_playlist_deletion_cascades_items(
    auth_client: TestClient, seeded_track_id: int, db_session: Session
) -> None:
    from app.models.playlist import PlaylistItem

    pid = auth_client.post("/api/playlists", json={"name": "cas"}).json()["id"]
    auth_client.post(
        f"/api/playlists/{pid}/tracks", json={"track_id": seeded_track_id}
    )
    db_session.expire_all()
    assert (
        db_session.scalar(select(PlaylistItem).where(PlaylistItem.playlist_id == pid))
        is not None
    )

    auth_client.delete(f"/api/playlists/{pid}")
    db_session.expire_all()
    assert (
        db_session.scalar(select(PlaylistItem).where(PlaylistItem.playlist_id == pid))
        is None
    )


# --- export -----------------------------------------------------------------


def test_export_m3u_lists_track_paths(
    auth_client: TestClient, seeded_track_id: int, extra_seeded_track_ids: list[int]
) -> None:
    pid = auth_client.post(
        "/api/playlists", json={"name": "Export Demo / Combat"}
    ).json()["id"]
    for tid in [seeded_track_id, *extra_seeded_track_ids]:
        auth_client.post(f"/api/playlists/{pid}/tracks", json={"track_id": tid})

    r = auth_client.get(f"/api/playlists/{pid}/export", params={"format": "m3u"})
    assert r.status_code == 200
    assert r.headers["content-type"].startswith("application/vnd.apple.mpegurl")
    disposition = r.headers["content-disposition"]
    assert ".m3u8" in disposition
    # Filename was sanitised — no spaces or slashes leaked through.
    assert "/" not in disposition.split("filename=")[1]

    body = r.text
    assert body.startswith("#EXTM3U")
    assert "#PLAYLIST:Export Demo / Combat" in body
    # Each track shows up as #EXTINF + relative path.
    assert "Demo/test-song.wav" in body
    assert body.count("#EXTINF:") == 4


def test_export_json_includes_metadata(
    auth_client: TestClient, seeded_track_id: int
) -> None:
    pid = auth_client.post(
        "/api/playlists", json={"name": "JSON Test"}
    ).json()["id"]
    auth_client.post(
        f"/api/playlists/{pid}/tracks", json={"track_id": seeded_track_id}
    )

    r = auth_client.get(f"/api/playlists/{pid}/export", params={"format": "json"})
    assert r.status_code == 200
    assert r.headers["content-type"].startswith("application/json")
    body = r.json()
    assert body["playlist"]["name"] == "JSON Test"
    assert len(body["tracks"]) == 1
    assert body["tracks"][0]["path"] == "Demo/test-song.wav"


def test_export_unknown_format_rejected(
    auth_client: TestClient, seeded_track_id: int
) -> None:
    pid = auth_client.post("/api/playlists", json={"name": "X"}).json()["id"]
    auth_client.post(
        f"/api/playlists/{pid}/tracks", json={"track_id": seeded_track_id}
    )
    r = auth_client.get(f"/api/playlists/{pid}/export", params={"format": "xml"})
    assert r.status_code == 422


def test_export_requires_auth(client: TestClient) -> None:
    r = client.get("/api/playlists/1/export")
    assert r.status_code == 401


# --- track-delete cascade regression ----------------------------------------


def test_track_delete_cascade_then_add_stays_contiguous(
    auth_client: TestClient,
    seeded_track_id: int,
    extra_seeded_track_ids: list[int],
    db_session: Session,
) -> None:
    """A Track row removed by the indexer (its file vanished) cascade-deletes
    the referencing PlaylistItem at the SQLite level (FK ondelete=CASCADE),
    bypassing `_shift_down` and leaving a position gap like [0, 2, 3]. The next
    `add_track(position=None)` derived its target from a COUNT of 3 and collided
    with the surviving position-3 row → IntegrityError on the composite PK. The
    domain now re-packs to contiguous on every read/write, so the gap closes and
    the append succeeds.
    """
    from app.library import index as library_index
    from app.models.track import Track

    pid = auth_client.post("/api/playlists", json={"name": "cascade"}).json()["id"]
    a, b, c = extra_seeded_track_ids
    for tid in [seeded_track_id, a, b, c]:
        auth_client.post(f"/api/playlists/{pid}/tracks", json={"track_id": tid})
    rows = auth_client.get(f"/api/playlists/{pid}/tracks").json()
    assert [r["position"] for r in rows] == [0, 1, 2, 3]

    # Delete the in-playlist track at position 1 via the indexer path → the DB
    # cascade drops its item, leaving positions [0, 2, 3].
    victim = db_session.get(Track, a)
    assert victim is not None
    assert library_index.remove_path(db_session, victim.path) is True
    db_session.expire_all()
    assert db_session.get(Track, a) is None

    # Used to 500 on an IntegrityError (target 3 collided with the surviving row).
    r = auth_client.post(
        f"/api/playlists/{pid}/tracks", json={"track_id": seeded_track_id}
    )
    assert r.status_code == 201, r.text

    rows = auth_client.get(f"/api/playlists/{pid}/tracks").json()
    positions = [row["position"] for row in rows]
    assert positions == list(range(len(rows)))  # contiguous 0..N-1
    assert positions == [0, 1, 2, 3]
    # Survivors keep their order; the re-added track lands at the tail.
    assert [row["track_id"] for row in rows] == [seeded_track_id, b, c, seeded_track_id]


# --- move_track regression: no-op + sentinel flip-back ----------------------


def test_move_track_noop_and_sentinel_fully_flipped_back(
    auth_client: TestClient,
    extra_seeded_track_ids: list[int],
    db_session: Session,
) -> None:
    """Locks two move_track invariants:

    (1) Moving an item onto its own position is a no-op — order and positions
        stay [0,1,2] (the `from_pos == to_pos` early return).
    (2) After a real move (0 -> 2), every row is back at a contiguous,
        non-negative position. A regression in the negation flip-back pass
        would strand a row at the `_PARKED_POSITION` sentinel (-1_000_000) or
        leave a stale negative value behind.
    """
    from app.domain import playlists as playlists_domain
    from app.models.playlist import PlaylistItem

    pid = auth_client.post("/api/playlists", json={"name": "sentinel"}).json()["id"]
    a, b, c = extra_seeded_track_ids
    for tid in [a, b, c]:
        auth_client.post(f"/api/playlists/{pid}/tracks", json={"track_id": tid})

    # (1) Move the middle item to its own position — no-op early return.
    assert auth_client.patch(
        f"/api/playlists/{pid}/tracks/1", json={"to_position": 1}
    ).status_code == 204
    rows = auth_client.get(f"/api/playlists/{pid}/tracks").json()
    assert [r["position"] for r in rows] == [0, 1, 2]
    assert [r["track_id"] for r in rows] == [a, b, c]

    # (2) Move position 0 to position 2, then inspect the DB rows directly.
    assert auth_client.patch(
        f"/api/playlists/{pid}/tracks/0", json={"to_position": 2}
    ).status_code == 204

    db_session.expire_all()
    items = db_session.scalars(
        select(PlaylistItem)
        .where(PlaylistItem.playlist_id == pid)
        .order_by(PlaylistItem.position)
    ).all()
    positions = [it.position for it in items]
    # Exactly the contiguous set {0,1,2} — no sentinel / stale negative left.
    assert positions == [0, 1, 2]
    assert all(it.position >= 0 for it in items)
    assert playlists_domain._PARKED_POSITION not in positions
    # Order resolved as expected (a moved to the tail).
    assert [it.track_id for it in items] == [b, c, a]
