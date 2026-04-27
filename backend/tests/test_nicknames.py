"""Nickname resolution + CRUD coverage.

Domain unit tests exercise the pure resolver against real DB rows. API
tests round-trip through HTTP. Cascade test verifies the FK pragma fix
actually deletes nickname rows when a playlist is removed.
"""
from fastapi.testclient import TestClient
from sqlalchemy import select
from sqlalchemy.orm import Session


def test_overview_requires_auth(client: TestClient, seeded_track_id: int) -> None:
    response = client.get(f"/api/nicknames/{seeded_track_id}")
    assert response.status_code == 401


def test_overview_404_for_unknown_beets_id(auth_client: TestClient) -> None:
    response = auth_client.get("/api/nicknames/999999")
    assert response.status_code == 404


def test_overview_empty_when_no_nicknames(auth_client: TestClient, seeded_track_id: int) -> None:
    response = auth_client.get(f"/api/nicknames/{seeded_track_id}")
    assert response.status_code == 200
    body = response.json()
    assert body["beets_id"] == seeded_track_id
    assert body["title"] == "Test Song"
    assert body["global"] is None
    assert body["modes"] == []
    assert body["playlists"] == []


def test_set_global_creates_then_updates(
    auth_client: TestClient, seeded_track_id: int
) -> None:
    r = auth_client.put(
        f"/api/nicknames/{seeded_track_id}/global", json={"display_name": "First"}
    )
    assert r.status_code == 204
    body = auth_client.get(f"/api/nicknames/{seeded_track_id}").json()
    assert body["global"] == "First"

    r = auth_client.put(
        f"/api/nicknames/{seeded_track_id}/global", json={"display_name": "Second"}
    )
    assert r.status_code == 204
    body = auth_client.get(f"/api/nicknames/{seeded_track_id}").json()
    assert body["global"] == "Second"


def test_delete_global_is_idempotent(auth_client: TestClient, seeded_track_id: int) -> None:
    auth_client.put(
        f"/api/nicknames/{seeded_track_id}/global", json={"display_name": "Tmp"}
    )
    r = auth_client.delete(f"/api/nicknames/{seeded_track_id}/global")
    assert r.status_code == 204
    # second delete: still 204, no error
    r = auth_client.delete(f"/api/nicknames/{seeded_track_id}/global")
    assert r.status_code == 204
    body = auth_client.get(f"/api/nicknames/{seeded_track_id}").json()
    assert body["global"] is None


def test_set_global_rejects_empty_display_name(
    auth_client: TestClient, seeded_track_id: int
) -> None:
    r = auth_client.put(
        f"/api/nicknames/{seeded_track_id}/global", json={"display_name": ""}
    )
    assert r.status_code == 422


def test_set_global_404_for_unknown_track(auth_client: TestClient) -> None:
    r = auth_client.put("/api/nicknames/999999/global", json={"display_name": "X"})
    assert r.status_code == 404


def test_set_mode_creates_and_lists(
    auth_client: TestClient, seeded_track_id: int
) -> None:
    r = auth_client.put(
        f"/api/nicknames/{seeded_track_id}/modes/dnd", json={"display_name": "Battle"}
    )
    assert r.status_code == 204
    body = auth_client.get(f"/api/nicknames/{seeded_track_id}").json()
    assert body["modes"] == [{"scope_id": "dnd", "display_name": "Battle"}]


def test_set_mode_rejects_unknown_mode(
    auth_client: TestClient, seeded_track_id: int
) -> None:
    r = auth_client.put(
        f"/api/nicknames/{seeded_track_id}/modes/nope", json={"display_name": "X"}
    )
    assert r.status_code == 400


def test_set_playlist_404_for_unknown_playlist(
    auth_client: TestClient, seeded_track_id: int
) -> None:
    r = auth_client.put(
        f"/api/nicknames/{seeded_track_id}/playlists/999999",
        json={"display_name": "X"},
    )
    assert r.status_code == 404


def _make_playlist(db: Session, name: str = "Test") -> int:
    from app.models.playlist import Playlist

    pl = Playlist(name=name, source="manual")
    db.add(pl)
    db.commit()
    return pl.id


def test_set_playlist_creates_and_lists(
    auth_client: TestClient, seeded_track_id: int, db_session: Session
) -> None:
    pid = _make_playlist(db_session)
    r = auth_client.put(
        f"/api/nicknames/{seeded_track_id}/playlists/{pid}",
        json={"display_name": "Ambush"},
    )
    assert r.status_code == 204
    body = auth_client.get(f"/api/nicknames/{seeded_track_id}").json()
    assert body["playlists"] == [{"scope_id": pid, "display_name": "Ambush"}]


def test_playlist_deletion_cascades_nicknames(
    auth_client: TestClient, seeded_track_id: int, db_session: Session
) -> None:
    from app.models.nickname import PlaylistNickname
    from app.models.playlist import Playlist

    pid = _make_playlist(db_session, name="ToDelete")
    auth_client.put(
        f"/api/nicknames/{seeded_track_id}/playlists/{pid}",
        json={"display_name": "Doomed"},
    )

    nick_count_before = db_session.scalar(
        select(PlaylistNickname).where(PlaylistNickname.playlist_id == pid)
    )
    assert nick_count_before is not None

    db_session.delete(db_session.get(Playlist, pid))
    db_session.commit()

    nick_after = db_session.scalar(
        select(PlaylistNickname).where(PlaylistNickname.playlist_id == pid)
    )
    assert nick_after is None, "PlaylistNickname row should have been cascaded out"


# --- Domain unit tests for the pure resolver --------------------------------


def test_resolve_returns_beets_title_when_no_nicknames(
    db_session: Session, seeded_track_id: int
) -> None:
    from app.domain.nicknames import resolve_name

    assert resolve_name(db_session, seeded_track_id) == "Test Song"


def test_resolve_returns_global_when_set(
    auth_client: TestClient, db_session: Session, seeded_track_id: int
) -> None:
    from app.domain.nicknames import resolve_name

    auth_client.put(
        f"/api/nicknames/{seeded_track_id}/global", json={"display_name": "GLOBAL"}
    )
    db_session.expire_all()
    assert resolve_name(db_session, seeded_track_id) == "GLOBAL"
    auth_client.delete(f"/api/nicknames/{seeded_track_id}/global")


def test_resolve_mode_wins_over_global(
    auth_client: TestClient, db_session: Session, seeded_track_id: int
) -> None:
    from app.domain.nicknames import resolve_name

    auth_client.put(
        f"/api/nicknames/{seeded_track_id}/global", json={"display_name": "GLOBAL"}
    )
    auth_client.put(
        f"/api/nicknames/{seeded_track_id}/modes/dnd", json={"display_name": "MODE"}
    )
    db_session.expire_all()
    assert resolve_name(db_session, seeded_track_id, mode_id="dnd") == "MODE"
    # without mode context, global still wins
    assert resolve_name(db_session, seeded_track_id) == "GLOBAL"

    auth_client.delete(f"/api/nicknames/{seeded_track_id}/global")
    auth_client.delete(f"/api/nicknames/{seeded_track_id}/modes/dnd")


def test_resolve_playlist_wins_over_mode_and_global(
    auth_client: TestClient, db_session: Session, seeded_track_id: int
) -> None:
    from app.domain.nicknames import resolve_name

    pid = _make_playlist(db_session, name="ResolveTest")
    auth_client.put(
        f"/api/nicknames/{seeded_track_id}/global", json={"display_name": "G"}
    )
    auth_client.put(
        f"/api/nicknames/{seeded_track_id}/modes/dnd", json={"display_name": "M"}
    )
    auth_client.put(
        f"/api/nicknames/{seeded_track_id}/playlists/{pid}",
        json={"display_name": "P"},
    )
    db_session.expire_all()
    assert (
        resolve_name(db_session, seeded_track_id, mode_id="dnd", playlist_id=pid) == "P"
    )

    auth_client.delete(f"/api/nicknames/{seeded_track_id}/global")
    auth_client.delete(f"/api/nicknames/{seeded_track_id}/modes/dnd")


def test_resolve_returns_none_for_orphaned_id(db_session: Session) -> None:
    from app.domain.nicknames import resolve_name

    assert resolve_name(db_session, 999999) is None


def test_resolve_names_bulk_walks_precedence(
    auth_client: TestClient, db_session: Session, seeded_track_id: int
) -> None:
    from app.domain.nicknames import resolve_names

    pid = _make_playlist(db_session, name="BulkTest")
    auth_client.put(
        f"/api/nicknames/{seeded_track_id}/playlists/{pid}",
        json={"display_name": "PLAYLIST"},
    )
    db_session.expire_all()

    result = resolve_names(
        db_session, [seeded_track_id, 999999], mode_id="dnd", playlist_id=pid
    )
    assert result == {seeded_track_id: "PLAYLIST", 999999: None}


def test_resolve_names_handles_empty_input(db_session: Session) -> None:
    from app.domain.nicknames import resolve_names

    assert resolve_names(db_session, []) == {}
