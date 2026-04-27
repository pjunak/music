"""Playlist CRUD + tracks endpoints + smart resolution + display name enrichment."""
from fastapi.testclient import TestClient
from sqlalchemy import select
from sqlalchemy.orm import Session

# --- playlist CRUD ----------------------------------------------------------


def test_create_manual_playlist(auth_client: TestClient) -> None:
    r = auth_client.post(
        "/api/playlists", json={"name": "Combat", "source": "manual", "mode_id": "dnd"}
    )
    assert r.status_code == 201
    body = r.json()
    assert body["name"] == "Combat"
    assert body["source"] == "manual"
    assert body["mode_id"] == "dnd"
    assert body["rules_json"] is None


def test_create_smart_playlist(auth_client: TestClient) -> None:
    r = auth_client.post(
        "/api/playlists",
        json={
            "name": "Daft 2001",
            "source": "smart",
            "rules_json": {"query": "artist:'Test Artist'"},
        },
    )
    assert r.status_code == 201
    assert r.json()["rules_json"] == {"query": "artist:'Test Artist'"}


def test_create_smart_requires_query(auth_client: TestClient) -> None:
    r = auth_client.post(
        "/api/playlists", json={"name": "Bad Smart", "source": "smart"}
    )
    assert r.status_code == 400
    r = auth_client.post(
        "/api/playlists",
        json={"name": "Bad Smart", "source": "smart", "rules_json": {"query": ""}},
    )
    assert r.status_code == 400


def test_create_validates_mode_id(auth_client: TestClient) -> None:
    r = auth_client.post(
        "/api/playlists", json={"name": "X", "source": "manual", "mode_id": "nope"}
    )
    assert r.status_code == 400


def test_list_filters_by_mode_includes_globals_by_default(auth_client: TestClient) -> None:
    auth_client.post(
        "/api/playlists",
        json={
            "name": "DnD-Combat",
            "source": "manual",
            "mode_id": "dnd",
            "category": "combat",
        },
    )
    auth_client.post(
        "/api/playlists",
        json={"name": "Global-Combat", "source": "manual", "category": "combat"},
    )

    # mode_id=dnd by default also includes global (mode_id=null) playlists.
    mixed = auth_client.get(
        "/api/playlists", params={"mode_id": "dnd", "category": "combat"}
    ).json()
    names = {p["name"] for p in mixed}
    assert "DnD-Combat" in names and "Global-Combat" in names

    # Strict mode-scoped lookup.
    strict = auth_client.get(
        "/api/playlists",
        params={"mode_id": "dnd", "category": "combat", "include_global": False},
    ).json()
    assert all(p["mode_id"] == "dnd" for p in strict)


def test_list_filters_by_source(auth_client: TestClient) -> None:
    auth_client.post(
        "/api/playlists",
        json={"name": "S1", "source": "smart", "rules_json": {"query": "*"}},
    )
    only_smart = auth_client.get("/api/playlists", params={"source": "smart"}).json()
    assert all(p["source"] == "smart" for p in only_smart)


def test_create_and_update_accept_category(auth_client: TestClient) -> None:
    r = auth_client.post(
        "/api/playlists",
        json={"name": "Cat", "source": "manual", "category": "exploration"},
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
    pid = auth_client.post(
        "/api/playlists", json={"name": "old", "source": "manual"}
    ).json()["id"]
    r = auth_client.patch(
        f"/api/playlists/{pid}", json={"name": "new", "mode_id": "dnd"}
    )
    assert r.status_code == 200
    body = r.json()
    assert body["name"] == "new"
    assert body["mode_id"] == "dnd"


def test_update_validates_mode(auth_client: TestClient) -> None:
    pid = auth_client.post(
        "/api/playlists", json={"name": "x", "source": "manual"}
    ).json()["id"]
    r = auth_client.patch(f"/api/playlists/{pid}", json={"mode_id": "nope"})
    assert r.status_code == 400


def test_delete_playlist(auth_client: TestClient) -> None:
    pid = auth_client.post(
        "/api/playlists", json={"name": "doomed", "source": "manual"}
    ).json()["id"]
    assert auth_client.delete(f"/api/playlists/{pid}").status_code == 204
    assert auth_client.get(f"/api/playlists/{pid}").status_code == 404


# --- manual playlist tracks -------------------------------------------------


def test_add_track_appends_then_returns_in_order(
    auth_client: TestClient, seeded_track_id: int, extra_seeded_track_ids: list[int]
) -> None:
    pid = auth_client.post(
        "/api/playlists", json={"name": "ord", "source": "manual"}
    ).json()["id"]

    expected = [seeded_track_id, *extra_seeded_track_ids]
    for bid in expected:
        auth_client.post(f"/api/playlists/{pid}/tracks", json={"beets_id": bid})

    rows = auth_client.get(f"/api/playlists/{pid}/tracks").json()
    assert [r["position"] for r in rows] == [0, 1, 2, 3]
    assert [r["beets_id"] for r in rows] == expected


def test_add_track_at_position_shifts_others(
    auth_client: TestClient, extra_seeded_track_ids: list[int]
) -> None:
    pid = auth_client.post(
        "/api/playlists", json={"name": "shift", "source": "manual"}
    ).json()["id"]
    a, b, c = extra_seeded_track_ids
    auth_client.post(f"/api/playlists/{pid}/tracks", json={"beets_id": a})
    auth_client.post(f"/api/playlists/{pid}/tracks", json={"beets_id": c})
    # insert b at position 1
    auth_client.post(
        f"/api/playlists/{pid}/tracks", json={"beets_id": b, "position": 1}
    )
    rows = auth_client.get(f"/api/playlists/{pid}/tracks").json()
    assert [r["beets_id"] for r in rows] == [a, b, c]
    assert [r["position"] for r in rows] == [0, 1, 2]


def test_add_track_validates_beets_id(auth_client: TestClient) -> None:
    pid = auth_client.post(
        "/api/playlists", json={"name": "x", "source": "manual"}
    ).json()["id"]
    r = auth_client.post(f"/api/playlists/{pid}/tracks", json={"beets_id": 999999})
    assert r.status_code == 404


def test_add_track_position_out_of_range(
    auth_client: TestClient, seeded_track_id: int
) -> None:
    pid = auth_client.post(
        "/api/playlists", json={"name": "x", "source": "manual"}
    ).json()["id"]
    # Empty playlist — position 5 is out of range.
    r = auth_client.post(
        f"/api/playlists/{pid}/tracks",
        json={"beets_id": seeded_track_id, "position": 5},
    )
    assert r.status_code == 400


def test_remove_track_shifts_down(
    auth_client: TestClient, extra_seeded_track_ids: list[int]
) -> None:
    pid = auth_client.post(
        "/api/playlists", json={"name": "rm", "source": "manual"}
    ).json()["id"]
    a, b, c = extra_seeded_track_ids
    for bid in [a, b, c]:
        auth_client.post(f"/api/playlists/{pid}/tracks", json={"beets_id": bid})

    assert auth_client.delete(f"/api/playlists/{pid}/tracks/0").status_code == 204
    rows = auth_client.get(f"/api/playlists/{pid}/tracks").json()
    assert [r["beets_id"] for r in rows] == [b, c]
    assert [r["position"] for r in rows] == [0, 1]


def test_remove_track_404_for_missing_position(auth_client: TestClient) -> None:
    pid = auth_client.post(
        "/api/playlists", json={"name": "x", "source": "manual"}
    ).json()["id"]
    assert auth_client.delete(f"/api/playlists/{pid}/tracks/0").status_code == 404


def test_move_track_forward(
    auth_client: TestClient, extra_seeded_track_ids: list[int]
) -> None:
    pid = auth_client.post(
        "/api/playlists", json={"name": "mv", "source": "manual"}
    ).json()["id"]
    a, b, c = extra_seeded_track_ids
    for bid in [a, b, c]:
        auth_client.post(f"/api/playlists/{pid}/tracks", json={"beets_id": bid})

    # Move position 0 (a) to position 2.
    r = auth_client.patch(
        f"/api/playlists/{pid}/tracks/0", json={"to_position": 2}
    )
    assert r.status_code == 204
    rows = auth_client.get(f"/api/playlists/{pid}/tracks").json()
    assert [r["beets_id"] for r in rows] == [b, c, a]


def test_move_track_backward(
    auth_client: TestClient, extra_seeded_track_ids: list[int]
) -> None:
    pid = auth_client.post(
        "/api/playlists", json={"name": "mvb", "source": "manual"}
    ).json()["id"]
    a, b, c = extra_seeded_track_ids
    for bid in [a, b, c]:
        auth_client.post(f"/api/playlists/{pid}/tracks", json={"beets_id": bid})

    # Move position 2 (c) to position 0.
    auth_client.patch(f"/api/playlists/{pid}/tracks/2", json={"to_position": 0})
    rows = auth_client.get(f"/api/playlists/{pid}/tracks").json()
    assert [r["beets_id"] for r in rows] == [c, a, b]


# --- smart playlists --------------------------------------------------------


def test_smart_playlist_resolves_query(auth_client: TestClient) -> None:
    pid = auth_client.post(
        "/api/playlists",
        json={
            "name": "ExtraOnly",
            "source": "smart",
            "rules_json": {"query": "artist:'Extra Artist'"},
        },
    ).json()["id"]
    rows = auth_client.get(f"/api/playlists/{pid}/tracks").json()
    titles = {r["track"]["title"] for r in rows}
    # The 3 extra-seeded tracks all have artist "Extra Artist".
    assert titles == {"Extra Song 2", "Extra Song 3", "Extra Song 4"}


def test_smart_playlist_rejects_track_writes(auth_client: TestClient) -> None:
    pid = auth_client.post(
        "/api/playlists",
        json={"name": "S", "source": "smart", "rules_json": {"query": "*"}},
    ).json()["id"]
    r = auth_client.post(f"/api/playlists/{pid}/tracks", json={"beets_id": 1})
    assert r.status_code == 400
    r = auth_client.delete(f"/api/playlists/{pid}/tracks/0")
    assert r.status_code == 400
    r = auth_client.patch(
        f"/api/playlists/{pid}/tracks/0", json={"to_position": 1}
    )
    assert r.status_code == 400


# --- display name enrichment via nickname resolution ------------------------


def test_tracks_use_beets_title_when_no_nicknames(
    auth_client: TestClient, seeded_track_id: int
) -> None:
    pid = auth_client.post(
        "/api/playlists", json={"name": "n1", "source": "manual"}
    ).json()["id"]
    auth_client.post(f"/api/playlists/{pid}/tracks", json={"beets_id": seeded_track_id})
    rows = auth_client.get(f"/api/playlists/{pid}/tracks").json()
    assert rows[0]["display_name"] == "Test Song"


def test_tracks_use_global_nickname(
    auth_client: TestClient, seeded_track_id: int
) -> None:
    pid = auth_client.post(
        "/api/playlists", json={"name": "n2", "source": "manual"}
    ).json()["id"]
    auth_client.post(f"/api/playlists/{pid}/tracks", json={"beets_id": seeded_track_id})
    auth_client.put(
        f"/api/nicknames/{seeded_track_id}/global", json={"display_name": "GG"}
    )
    rows = auth_client.get(f"/api/playlists/{pid}/tracks").json()
    assert rows[0]["display_name"] == "GG"
    auth_client.delete(f"/api/nicknames/{seeded_track_id}/global")


def test_tracks_use_mode_nickname_when_playlist_has_mode(
    auth_client: TestClient, seeded_track_id: int
) -> None:
    pid = auth_client.post(
        "/api/playlists",
        json={"name": "n3", "source": "manual", "mode_id": "dnd"},
    ).json()["id"]
    auth_client.post(f"/api/playlists/{pid}/tracks", json={"beets_id": seeded_track_id})
    auth_client.put(
        f"/api/nicknames/{seeded_track_id}/global", json={"display_name": "GG"}
    )
    auth_client.put(
        f"/api/nicknames/{seeded_track_id}/modes/dnd", json={"display_name": "MM"}
    )
    rows = auth_client.get(f"/api/playlists/{pid}/tracks").json()
    assert rows[0]["display_name"] == "MM"
    auth_client.delete(f"/api/nicknames/{seeded_track_id}/global")
    auth_client.delete(f"/api/nicknames/{seeded_track_id}/modes/dnd")


def test_tracks_use_playlist_nickname_overrides(
    auth_client: TestClient, seeded_track_id: int
) -> None:
    pid = auth_client.post(
        "/api/playlists",
        json={"name": "n4", "source": "manual", "mode_id": "dnd"},
    ).json()["id"]
    auth_client.post(f"/api/playlists/{pid}/tracks", json={"beets_id": seeded_track_id})
    auth_client.put(
        f"/api/nicknames/{seeded_track_id}/global", json={"display_name": "GLOBAL"}
    )
    auth_client.put(
        f"/api/nicknames/{seeded_track_id}/modes/dnd", json={"display_name": "MODE"}
    )
    auth_client.put(
        f"/api/nicknames/{seeded_track_id}/playlists/{pid}",
        json={"display_name": "PLAYLIST"},
    )
    rows = auth_client.get(f"/api/playlists/{pid}/tracks").json()
    assert rows[0]["display_name"] == "PLAYLIST"

    auth_client.delete(f"/api/nicknames/{seeded_track_id}/global")
    auth_client.delete(f"/api/nicknames/{seeded_track_id}/modes/dnd")


def test_playlist_deletion_cascades_items(
    auth_client: TestClient, seeded_track_id: int, db_session: Session
) -> None:
    from app.models.playlist import PlaylistItem

    pid = auth_client.post(
        "/api/playlists", json={"name": "cas", "source": "manual"}
    ).json()["id"]
    auth_client.post(f"/api/playlists/{pid}/tracks", json={"beets_id": seeded_track_id})
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
