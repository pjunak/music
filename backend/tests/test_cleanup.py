"""Cleanup engine + API tests.

Engine tests drive `app.library.cleanup.analyze` with fake row objects that
mimic the indexer's fallback semantics (title := stem, album := parent
folder when tags are absent). API tests run the full loop on real silent
WAVs: analyze → apply → verify disk+index → revert → verify restored.
"""
from __future__ import annotations

import os
from dataclasses import dataclass
from pathlib import Path, PurePosixPath

from fastapi.testclient import TestClient

from app.library import cleanup
from tests.conftest import _silent_wav_bytes

# --- engine fixtures --------------------------------------------------------


@dataclass
class Row:
    id: int
    path: str
    title: str = ""
    artist: str = ""
    album_artist: str = ""
    album: str = ""
    track_no: int | None = None
    disc_no: int | None = None
    year: int | None = None


_next_id = iter(range(1, 10_000))


def row(path: str, **kw) -> Row:
    """Build a fake Track row with the indexer's fallback behaviour: when a
    tag is absent the row carries title=stem / album=parent-folder-name."""
    p = PurePosixPath(path)
    parent = "" if str(p.parent) == "." else str(p.parent)
    defaults = {
        "title": p.stem,
        "album": PurePosixPath(parent).name if parent else "",
    }
    defaults.update(kw)
    return Row(id=next(_next_id), path=path, **defaults)


def plan_for(plans: list[cleanup.TrackPlan], r: Row) -> cleanup.TrackPlan | None:
    return next((p for p in plans if p.track_id == r.id), None)


def op(plan: cleanup.TrackPlan | None, kind: str, field: str | None = None):
    if plan is None:
        return None
    return next((o for o in plan.ops if o.kind == kind and o.field == field), None)


# --- engine: track numbers ----------------------------------------------------


def test_corroborated_numbered_album():
    rows = [
        row("Album/01 - Alpha.mp3"),
        row("Album/02 - Beta.mp3"),
        row("Album/03 - Gamma.mp3"),
    ]
    plans = cleanup.analyze(rows, rows)
    for r, expected_no, name in zip(rows, (1, 2, 3), ("Alpha", "Beta", "Gamma"), strict=True):
        p = plan_for(plans, r)
        rename = op(p, "rename")
        assert rename is not None and rename.new == name and rename.confidence == "high"
        title = op(p, "tag", "title")
        assert title is not None and title.new == name
        number = op(p, "tag", "track_no")
        assert number is not None and number.new == expected_no and number.confidence == "high"


def test_bare_number_uncorroborated_is_low_confidence():
    r = row("Misc/99 Luftballons.mp3")
    plans = cleanup.analyze([r], [r])
    rename = op(plan_for(plans, r), "rename")
    assert rename is not None and rename.new == "Luftballons"
    assert rename.confidence == "low"


def test_pure_numeric_stem_is_never_stripped():
    r = row("Misc/1979.mp3")
    plans = cleanup.analyze([r], [r])
    assert plan_for(plans, r) is None


def test_dash_separated_number_is_high_confidence_alone():
    r = row("Misc/05 - Foo.mp3")
    plans = cleanup.analyze([r], [r])
    p = plan_for(plans, r)
    rename = op(p, "rename")
    assert rename is not None and rename.new == "Foo" and rename.confidence == "high"
    # Stripping is confident (dash form), but with no sibling sequence the
    # *track number* claim is only a guess — suggested, default-unticked.
    number = op(p, "tag", "track_no")
    assert number is not None and number.new == 5 and number.confidence == "low"


def test_duplicate_numbers_strip_but_dont_tag_confidently():
    # A folder of unrelated "01 - X" singles: every number is 01, so it's
    # not a sequence — strip with confidence, tag only as a low guess.
    rows = [row("Singles/01 - Alpha.mp3"), row("Singles/01 - Beta.mp3")]
    plans = cleanup.analyze(rows, rows)
    for r in rows:
        p = plan_for(plans, r)
        assert op(p, "rename").confidence == "high"
        assert op(p, "tag", "track_no").confidence == "low"


def test_disc_track_pair():
    rows = [row("Box/1-01 - One.mp3"), row("Box/1-02 - Two.mp3")]
    plans = cleanup.analyze(rows, rows)
    p = plan_for(plans, rows[0])
    assert op(p, "rename").new == "One"
    assert op(p, "tag", "track_no").new == 1
    assert op(p, "tag", "disc_no").new == 1


def test_year_prefix_is_not_a_track_number():
    r = row("Misc/2017 - Slow Burn.mp3")
    plans = cleanup.analyze([r], [r])
    rename = op(plan_for(plans, r), "rename")
    # 4-digit leading numbers are years; the name must keep them.
    assert rename is None or rename.new.startswith("2017")


# --- engine: artist / album segments ------------------------------------------


def test_artist_tag_match_strips_segment():
    r = row(
        "Rock/Queen - Bohemian Rhapsody.mp3",
        title="Bohemian Rhapsody",
        artist="Queen",
        album="A Night at the Opera",
    )
    plans = cleanup.analyze([r], [r])
    p = plan_for(plans, r)
    rename = op(p, "rename")
    assert rename is not None and rename.new == "Bohemian Rhapsody"
    assert rename.confidence == "high"
    assert op(p, "tag", "artist") is None  # already tagged
    assert op(p, "tag", "title") is None  # already clean


def test_shared_segment_suggests_artist():
    rows = [
        row("Random/Daft Punk - One More Time.mp3"),
        row("Random/Daft Punk - Aerodynamic.mp3"),
        row("Random/Daft Punk - Digital Love.mp3"),
    ]
    plans = cleanup.analyze(rows, rows)
    p = plan_for(plans, rows[0])
    rename = op(p, "rename")
    assert rename is not None and rename.new == "One More Time"
    artist = op(p, "tag", "artist")
    assert artist is not None and artist.new == "Daft Punk"


def test_artist_album_number_combo():
    rows = [
        row("Stuff/Artist - Album - 01 - Song One.mp3"),
        row("Stuff/Artist - Album - 02 - Song Two.mp3"),
        row("Stuff/Artist - Album - 03 - Song Three.mp3"),
    ]
    plans = cleanup.analyze(rows, rows)
    p = plan_for(plans, rows[1])
    assert op(p, "rename").new == "Song Two"
    assert op(p, "tag", "artist").new == "Artist"
    assert op(p, "tag", "album").new == "Album"
    assert op(p, "tag", "track_no").new == 2
    assert op(p, "tag", "title").new == "Song Two"


def test_folder_name_segment_strips():
    rows = [
        row("Skyrim/Skyrim - Dragonborn.mp3"),
        row("Skyrim/Skyrim - Secunda.mp3"),
    ]
    plans = cleanup.analyze(rows, rows)
    rename = op(plan_for(plans, rows[0]), "rename")
    assert rename is not None and rename.new == "Dragonborn"


def test_dashed_title_without_signals_is_kept():
    # Artist tag set and doesn't match the first segment: the dash is part
    # of the actual title, not residue.
    r = row("Misc/Sgt Pepper - Reprise.mp3", title="Sgt Pepper - Reprise", artist="The Beatles")
    plans = cleanup.analyze([r], [r])
    assert plan_for(plans, r) is None


def test_lone_pair_guess_is_low_confidence():
    r = row("Misc/Unknown Artist - Some Song.mp3")
    plans = cleanup.analyze([r], [r])
    p = plan_for(plans, r)
    rename = op(p, "rename")
    artist = op(p, "tag", "artist")
    assert rename is not None and rename.confidence == "low"
    assert artist is not None and artist.new == "Unknown Artist" and artist.confidence == "low"


def test_accent_and_case_insensitive_artist_match():
    r = row("Pop/beyonce - Halo.mp3", title="Halo", artist="Beyoncé")
    plans = cleanup.analyze([r], [r])
    rename = op(plan_for(plans, r), "rename")
    assert rename is not None and rename.new == "Halo"


# --- engine: junk, separators, case -------------------------------------------


def test_junk_phrases_stripped_meaningful_parens_kept():
    r1 = row("Misc/Song Title (Official Audio) [320kbps].mp3")
    r2 = row("Misc/Song Two (Live).mp3")
    r3 = row("Misc/[www.mp3crazy.ru] Song Three.mp3")
    plans = cleanup.analyze([r1, r2, r3], [r1, r2, r3])
    assert op(plan_for(plans, r1), "rename").new == "Song Title"
    assert plan_for(plans, r2) is None  # "(Live)" is meaningful — untouched
    assert op(plan_for(plans, r3), "rename").new == "Song Three"


def test_underscores_become_spaces():
    r = row("Misc/Some_Track_Name.mp3")
    plans = cleanup.analyze([r], [r])
    assert op(plan_for(plans, r), "rename").new == "Some Track Name"


def test_case_rule_is_off_by_default():
    r = row("Misc/MY LOUD SONG.mp3")
    assert plan_for(cleanup.analyze([r], [r]), r) is None
    plans = cleanup.analyze([r], [r], cleanup.ALL_RULES)
    rename = op(plan_for(plans, r), "rename")
    assert rename is not None and rename.new == "My Loud Song"


def test_rule_gating_only_junk():
    r = row("Misc/01 - Foo (Official Audio).mp3")
    plans = cleanup.analyze([r], [r], {"strip_junk"})
    p = plan_for(plans, r)
    rename = op(p, "rename")
    assert rename is not None and rename.new == "01 - Foo"
    assert [o for o in p.ops if o.kind == "tag"] == []  # tag rules disabled


def test_title_tag_with_junk_is_cleaned_even_when_filename_is_clean():
    r = row("Misc/Clean Name.mp3", title="Clean Name (Official Video)")
    plans = cleanup.analyze([r], [r])
    p = plan_for(plans, r)
    assert op(p, "rename") is None
    title = op(p, "tag", "title")
    assert title is not None and title.new == "Clean Name"


# --- engine: folder-structure clues --------------------------------------------


def test_suspect_album_equal_to_artist_gets_folder_album():
    # Rip bug: the album field holds the artist's name. The folder name is
    # the real album and must not be ignored just because album is "set".
    rows = [
        row(
            "Hurdy-Gurdy Meditations/01 - Pastorale.mp3",
            title="Pastorale",
            artist="Andrey Vinogradov",
            album="Andrey Vinogradov",
            track_no=1,
        ),
        row(
            "Hurdy-Gurdy Meditations/02 - Oberek.mp3",
            title="Oberek",
            artist="Andrey Vinogradov",
            album="Andrey Vinogradov",
            track_no=2,
        ),
    ]
    plans = cleanup.analyze(rows, rows)
    album = op(plan_for(plans, rows[0]), "tag", "album")
    assert album is not None
    assert album.old == "Andrey Vinogradov"
    assert album.new == "Hurdy-Gurdy Meditations"
    assert album.confidence == "high"  # numbered sequence makes the folder albumish


def test_self_titled_album_in_artist_folder_left_alone():
    # artist == album == folder: could be a legit self-titled album, and the
    # folder offers no *different* name anyway — no suggestion.
    r = row("Weezer/Buddy Holly.mp3", title="Buddy Holly", artist="Weezer", album="Weezer")
    plans = cleanup.analyze([r], [r])
    assert plan_for(plans, r) is None


def test_disc_folder_layout():
    rows = [
        row("Big Album/CD1/01 - One.mp3"),
        row("Big Album/CD1/02 - Two.mp3"),
        row("Big Album/CD2/01 - Three.mp3"),
    ]
    plans = cleanup.analyze(rows, rows)
    p = plan_for(plans, rows[0])
    disc = op(p, "tag", "disc_no")
    assert disc is not None and disc.new == 1 and disc.confidence == "high"
    album = op(p, "tag", "album")
    assert album is not None
    assert album.old == "CD1"  # the row fallback was the disc folder
    assert album.new == "Big Album" and album.confidence == "high"
    assert op(plan_for(plans, rows[2]), "tag", "disc_no").new == 2


def test_artist_album_folder_label():
    rows = [
        row("Two Steps From Hell - Archangel/01 - Winterspell.mp3"),
        row("Two Steps From Hell - Archangel/02 - Dragon Rider.mp3"),
    ]
    plans = cleanup.analyze(rows, rows)
    p = plan_for(plans, rows[0])
    artist = op(p, "tag", "artist")
    assert artist is not None and artist.new == "Two Steps From Hell"
    assert artist.confidence == "low"  # folder label alone is a guess
    album = op(p, "tag", "album")
    assert album is not None and album.new == "Archangel"
    assert album.confidence == "high"  # numbered sequence → albumish


def test_year_marker_in_folder():
    rows = [
        row("Skyrim OST (2011)/01 - Dragonborn.mp3"),
        row("Skyrim OST (2011)/02 - Awake.mp3"),
    ]
    plans = cleanup.analyze(rows, rows)
    p = plan_for(plans, rows[0])
    year = op(p, "tag", "year")
    assert year is not None and year.new == 2011 and year.confidence == "high"
    album = op(p, "tag", "album")
    assert album is not None
    assert album.old == "Skyrim OST (2011)" and album.new == "Skyrim OST"

    # Rule gating: disabling tag_year drops the year op.
    plans = cleanup.analyze(rows, rows, cleanup.DEFAULT_RULES - {"tag_year"})
    assert op(plan_for(plans, rows[0]), "tag", "year") is None


def test_dominant_sibling_artist_and_album_fill_stragglers():
    rows = [
        row("Mixtape/Alpha.mp3", title="Alpha", artist="The Band", album="Live Set"),
        row("Mixtape/Beta.mp3", title="Beta", artist="The Band", album="Live Set"),
        row("Mixtape/Gamma.mp3", title="Gamma"),  # untagged straggler
    ]
    plans = cleanup.analyze(rows, rows)
    p = plan_for(plans, rows[2])
    artist = op(p, "tag", "artist")
    assert artist is not None and artist.new == "The Band" and artist.confidence == "high"
    album = op(p, "tag", "album")
    assert album is not None and album.new == "Live Set" and album.confidence == "high"
    # The already-tagged siblings get no suggestions.
    assert plan_for(plans, rows[0]) is None


def test_album_artist_tag_fills_artist():
    r = row("Misc/Halo.mp3", title="Halo", album_artist="Beyoncé")
    plans = cleanup.analyze([r], [r])
    artist = op(plan_for(plans, r), "tag", "artist")
    assert artist is not None and artist.new == "Beyoncé" and artist.confidence == "high"


def test_grandparent_artist_layout():
    rows = [
        row("Two Steps From Hell/Archangel/01 - Winterspell.mp3"),
        row("Two Steps From Hell/Archangel/02 - Dragon Rider.mp3"),
    ]
    plans = cleanup.analyze(rows, rows)
    p = plan_for(plans, rows[0])
    artist = op(p, "tag", "artist")
    assert artist is not None and artist.new == "Two Steps From Hell"
    assert artist.confidence == "low"  # structural guess, review confirms
    # The album candidate equals the folder name the row already displays
    # (fallback) — an invisible diff, so by design no op is proposed.
    assert op(p, "tag", "album") is None


def test_segment_matching_grandparent_strips_high():
    rows = [
        row("Daft Punk/Discovery/Daft Punk - One More Time.mp3"),
        row("Daft Punk/Discovery/Daft Punk - Aerodynamic.mp3"),
    ]
    plans = cleanup.analyze(rows, rows)
    p = plan_for(plans, rows[0])
    rename = op(p, "rename")
    assert rename is not None and rename.new == "One More Time"
    artist = op(p, "tag", "artist")
    assert artist is not None and artist.new == "Daft Punk" and artist.confidence == "high"


def test_generic_folder_names_never_suggested():
    rows = [row("Uploads/01 - Foo.mp3"), row("Uploads/02 - Bar.mp3")]
    plans = cleanup.analyze(rows, rows)
    p = plan_for(plans, rows[0])
    assert op(p, "rename") is not None  # number strip still works
    assert op(p, "tag", "album") is None
    assert op(p, "tag", "artist") is None


# --- engine: online name verdicts ----------------------------------------------


def verdict_map(names: dict[str, tuple[int, int]]) -> dict[str, tuple[int, int]]:
    """Verdict map from {name: (artist_score, album_score)}."""
    return {cleanup.loose_key(k): v for k, v in names.items()}


def test_verdict_kind_thresholds():
    assert cleanup.verdict_kind(100, 20) == "artist"
    assert cleanup.verdict_kind(30, 95) == "album"
    assert cleanup.verdict_kind(100, 100) == "both"  # self-titled albums exist
    assert cleanup.verdict_kind(92, 80) == "both"  # too close to call
    assert cleanup.verdict_kind(40, 50) == "unknown"


def test_verified_artist_upgrades_lone_pair():
    r = row("Misc/Andrey Vinogradov - Pastorale.mp3")
    verdicts = verdict_map({"Andrey Vinogradov": (100, 25)})
    plans = cleanup.analyze([r], [r], verdicts=verdicts)
    p = plan_for(plans, r)
    rename = op(p, "rename")
    assert rename is not None and rename.new == "Pastorale" and rename.confidence == "high"
    artist = op(p, "tag", "artist")
    assert artist is not None and artist.new == "Andrey Vinogradov"
    assert artist.confidence == "high" and artist.verified


def test_verified_album_classifies_lone_pair_as_album():
    r = row("Misc/Abbey Road - Come Together.mp3")
    verdicts = verdict_map({"Abbey Road": (10, 100)})
    plans = cleanup.analyze([r], [r], verdicts=verdicts)
    p = plan_for(plans, r)
    rename = op(p, "rename")
    assert rename is not None and rename.new == "Come Together"
    assert "strip_album" in rename.rules
    album = op(p, "tag", "album")
    assert album is not None and album.new == "Abbey Road"
    assert album.confidence == "high" and album.verified
    assert op(p, "tag", "artist") is None  # not mistaken for an artist


def test_verdict_flips_folder_album_candidate_to_artist():
    # Folder named after the artist, segment-less untagged files: offline
    # the folder name is an *invisible* album candidate (it equals the
    # row's fallback), so no op ships — but the name is queued for lookup,
    # and the verdict moves it to the artist field on the next pass.
    rows = [
        row("Andrey Vinogradov/01 - Pastorale.mp3"),
        row("Andrey Vinogradov/02 - Oberek.mp3"),
    ]
    offline = cleanup.analyze(rows, rows)
    off_p = plan_for(offline, rows[0])
    assert op(off_p, "tag", "artist") is None
    assert op(off_p, "tag", "album") is None
    assert cleanup.pending_lookups(offline) == ["Andrey Vinogradov"]

    verdicts = verdict_map({"Andrey Vinogradov": (100, 30)})
    plans = cleanup.analyze(rows, rows, verdicts=verdicts)
    p = plan_for(plans, rows[0])
    assert op(p, "tag", "album") is None  # wrong-side guess stays dropped
    artist = op(p, "tag", "artist")
    assert artist is not None and artist.new == "Andrey Vinogradov"
    assert artist.confidence == "high" and artist.verified
    assert cleanup.pending_lookups(plans, verdicts) == []


def test_verified_album_upgrades_folder_album_candidate():
    rows = [
        row("Hurdy-Gurdy Meditations/Pastorale.mp3", title="Pastorale"),
        row("Hurdy-Gurdy Meditations/Oberek.mp3", title="Oberek"),
    ]
    # Plain folder, no numbers/year: the candidate equals the fallback —
    # invisible, no visible ops; the name still queues for lookup.
    offline = cleanup.analyze(rows, rows)
    assert op(plan_for(offline, rows[0]), "tag", "album") is None
    assert "Hurdy-Gurdy Meditations" in cleanup.pending_lookups(offline)

    # With a suspect album tag the suggestion exists; the verdict makes it high.
    rows = [
        row("Hurdy-Gurdy Meditations/Pastorale.mp3", title="Pastorale", artist="X", album="X"),
        row("Hurdy-Gurdy Meditations/Oberek.mp3", title="Oberek", artist="X", album="X"),
    ]
    offline = cleanup.analyze(rows, rows)
    off_album = op(plan_for(offline, rows[0]), "tag", "album")
    assert off_album is not None and off_album.confidence == "low"

    verdicts = verdict_map({"Hurdy-Gurdy Meditations": (5, 100)})
    plans = cleanup.analyze(rows, rows, verdicts=verdicts)
    album = op(plan_for(plans, rows[0]), "tag", "album")
    assert album is not None and album.confidence == "high" and album.verified


def test_pending_lookups_dedupes_and_skips_known():
    rows = [
        row("Misc/Daft Punk - One.mp3"),
        row("Misc/Daft Punk - Two.mp3"),
        row("Misc/Daft Punk - Three.mp3"),
    ]
    plans = cleanup.analyze(rows, rows)
    pending = cleanup.pending_lookups(plans)
    assert pending == ["Daft Punk"]  # one entry for three tracks

    verdicts = verdict_map({"Daft Punk": (100, 30)})
    plans = cleanup.analyze(rows, rows, verdicts=verdicts)
    assert cleanup.pending_lookups(plans, verdicts) == []  # known + now high


# --- engine: collisions + scope ------------------------------------------------


def test_rename_collision_with_existing_file_is_dropped():
    target = row("Misc/Bar.mp3")
    r = row("Misc/01 - Bar.mp3")
    plans = cleanup.analyze([r], [r, target])
    p = plan_for(plans, r)
    assert op(p, "rename") is None
    assert any("already exists" in n for n in p.notes)


def test_rename_collision_between_two_proposals():
    r1 = row("Misc/01 - Foo.mp3")
    r2 = row("Misc/1. Foo.mp3")
    plans = cleanup.analyze([r1, r2], [r1, r2])
    renames = [op(plan_for(plans, r), "rename") for r in (r1, r2)]
    kept = [x for x in renames if x is not None]
    assert len(kept) == 1  # one wins, the other is dropped with a note
    noted = plan_for(plans, r2 if renames[0] is not None else r1)
    assert any("another track" in n for n in noted.notes)


def test_scoped_track_still_uses_folder_context():
    rows = [
        row("Album/01 - Alpha.mp3"),
        row("Album/02 - Beta.mp3"),
        row("Album/03 - Gamma.mp3"),
    ]
    # Scope only one track; siblings corroborate the number pattern.
    plans = cleanup.analyze([rows[2]], rows)
    rename = op(plan_for(plans, rows[2]), "rename")
    assert rename is not None and rename.confidence == "high"
    assert len(plans) == 1  # out-of-scope siblings get no plans


def test_clean_library_yields_no_plans():
    rows = [
        row("Album/Alpha.mp3", title="Alpha", artist="X", track_no=1),
        row("Album/Beta.mp3", title="Beta", artist="X", track_no=2),
    ]
    assert cleanup.analyze(rows, rows) == []


# --- API ----------------------------------------------------------------------


def _seed_folder(folder: str, names: list[str]) -> None:
    music_dir = Path(os.environ["MUSIC_DIR"])
    target = music_dir / folder
    target.mkdir(parents=True, exist_ok=True)
    paths = []
    for name in names:
        p = target / name
        p.write_bytes(_silent_wav_bytes())
        paths.append(p)
    from app.core.db import SessionLocal
    from app.library import index as library_index

    with SessionLocal() as db:
        library_index.scan_paths(db, paths)


def _ops_in(plans: list[dict]) -> list[dict]:
    return [
        {"track_id": o["track_id"], "kind": o["kind"], "field": o["field"], "old": o["old"], "new": o["new"]}
        for p in plans
        for o in p["ops"]
    ]


def test_cleanup_requires_auth(client: TestClient):
    r = client.post(
        "/api/library/cleanup/analyze", json={"scope": {"type": "all"}}
    )
    assert r.status_code == 401


def test_analyze_apply_revert_roundtrip(auth_client: TestClient):
    _seed_folder("CleanupRT", ["01 - Alpha.wav", "02 - Beta.wav"])

    r = auth_client.post(
        "/api/library/cleanup/analyze",
        json={"scope": {"type": "folder", "path": "CleanupRT", "recursive": True}},
    )
    assert r.status_code == 200, r.text
    plans = r.json()["plans"]
    assert len(plans) == 2
    ops = _ops_in(plans)
    # Tags are listed before the rename for each track (apply-order matters).
    kinds_first_track = [o["kind"] for o in plans[0]["ops"]]
    assert kinds_first_track[-1] == "rename" and "tag" in kinds_first_track

    r = auth_client.post(
        "/api/library/cleanup/apply",
        json={"ops": ops, "scope_label": "test roundtrip"},
    )
    assert r.status_code == 200, r.text
    body = r.json()
    assert body["skipped"] == []
    assert body["applied"] == len(ops)
    batch_id = body["batch_id"]
    assert batch_id is not None

    music_dir = Path(os.environ["MUSIC_DIR"])
    assert (music_dir / "CleanupRT" / "Alpha.wav").is_file()
    assert not (music_dir / "CleanupRT" / "01 - Alpha.wav").exists()

    # Index follows: renamed paths + written tags are visible.
    tree = auth_client.get("/api/library/tree?path=CleanupRT").json()
    by_name = {t["path"].split("/")[-1]: t for t in tree["tracks"]}
    assert set(by_name) == {"Alpha.wav", "Beta.wav"}
    assert by_name["Alpha.wav"]["track_no"] == 1
    assert by_name["Alpha.wav"]["title"] == "Alpha"

    # Idempotent: a second analysis finds nothing left to fix.
    r = auth_client.post(
        "/api/library/cleanup/analyze",
        json={"scope": {"type": "folder", "path": "CleanupRT", "recursive": True}},
    )
    assert r.json()["plans"] == []

    # Journal is listed and downloadable.
    batches = auth_client.get("/api/library/cleanup/batches").json()
    mine = next(b for b in batches if b["id"] == batch_id)
    assert mine["item_count"] == len(ops)
    assert mine["reverted_at"] is None
    detail = auth_client.get(f"/api/library/cleanup/batches/{batch_id}").json()
    assert len(detail["items"]) == len(ops)

    # Revert restores filenames and tags.
    r = auth_client.post(f"/api/library/cleanup/batches/{batch_id}/revert")
    assert r.status_code == 200, r.text
    assert r.json()["skipped"] == []
    assert (music_dir / "CleanupRT" / "01 - Alpha.wav").is_file()
    assert not (music_dir / "CleanupRT" / "Alpha.wav").exists()
    tree = auth_client.get("/api/library/tree?path=CleanupRT").json()
    by_name = {t["path"].split("/")[-1]: t for t in tree["tracks"]}
    assert by_name["01 - Alpha.wav"]["track_no"] is None

    # Perfect tag roundtrip: the seeded WAV had no tags, so after revert it
    # must have none again — the title written by apply is deleted, not
    # rewritten as the old visible (filename-fallback) value.
    from mutagen.wave import WAVE

    reverted_tags = WAVE(str(music_dir / "CleanupRT" / "01 - Alpha.wav")).tags
    assert not reverted_tags, f"expected no tags after revert, found {dict(reverted_tags)}"

    # A batch reverts once.
    r = auth_client.post(f"/api/library/cleanup/batches/{batch_id}/revert")
    assert r.status_code == 409


def test_apply_skips_stale_ops(auth_client: TestClient):
    _seed_folder("CleanupStale", ["03 - Gamma.wav"])
    r = auth_client.post(
        "/api/library/cleanup/analyze",
        json={"scope": {"type": "folder", "path": "CleanupStale"}},
    )
    plans = r.json()["plans"]
    ops = [o for o in _ops_in(plans) if o["kind"] == "rename"]
    assert ops

    # The file gets renamed between analyze and apply.
    track_id = ops[0]["track_id"]
    r = auth_client.post(
        f"/api/library/tracks/{track_id}/move",
        json={"destination": "CleanupStale", "new_filename": "renamed-elsewhere.wav"},
    )
    assert r.status_code == 200, r.text

    r = auth_client.post("/api/library/cleanup/apply", json={"ops": ops})
    body = r.json()
    assert body["applied"] == 0
    assert body["batch_id"] is None  # nothing landed — no journal row
    assert body["skipped"][0]["reason"] == "filename changed since analysis"


def test_apply_partial_selection_only_ticked_ops(auth_client: TestClient):
    _seed_folder("CleanupPartial", ["04 - Delta.wav", "05 - Echo.wav"])
    r = auth_client.post(
        "/api/library/cleanup/analyze",
        json={"scope": {"type": "folder", "path": "CleanupPartial"}},
    )
    ops = _ops_in(r.json()["plans"])
    only_numbers = [o for o in ops if o["field"] == "track_no"]
    assert len(only_numbers) == 2

    r = auth_client.post("/api/library/cleanup/apply", json={"ops": only_numbers})
    assert r.json()["applied"] == 2

    music_dir = Path(os.environ["MUSIC_DIR"])
    assert (music_dir / "CleanupPartial" / "04 - Delta.wav").is_file()  # not renamed
    tree = auth_client.get("/api/library/tree?path=CleanupPartial").json()
    assert {t["track_no"] for t in tree["tracks"]} == {4, 5}


def test_verify_endpoint_caches_and_feeds_next_analysis(auth_client: TestClient, monkeypatch):
    _seed_folder("CleanupVerify", ["Andrey Vinogradov - Pastorale.wav"])

    r = auth_client.post(
        "/api/library/cleanup/analyze",
        json={"scope": {"type": "folder", "path": "CleanupVerify"}},
    )
    body = r.json()
    assert "Andrey Vinogradov" in body["pending_lookups"]
    artist_op = next(
        o for p in body["plans"] for o in p["ops"] if o["field"] == "artist"
    )
    assert artist_op["confidence"] == "low" and not artist_op["verified"]

    calls: list[str] = []

    def fake_fetch(name: str) -> tuple[int, int]:
        calls.append(name)
        return (100, 20)  # clearly an artist

    monkeypatch.setattr("app.library.cleanup_lookup.fetch_name_scores", fake_fetch)

    r = auth_client.post(
        "/api/library/cleanup/verify", json={"names": body["pending_lookups"]}
    )
    assert r.status_code == 200, r.text
    assert r.json()["verified"] == len(body["pending_lookups"])
    assert r.json()["failed"] == []

    # Cached: a second verify never touches the network again.
    before = len(calls)
    r = auth_client.post(
        "/api/library/cleanup/verify", json={"names": ["Andrey Vinogradov"]}
    )
    assert r.json()["verified"] == 0
    assert len(calls) == before

    # The next analysis picks the verdicts up: upgraded, verified, settled.
    r = auth_client.post(
        "/api/library/cleanup/analyze",
        json={"scope": {"type": "folder", "path": "CleanupVerify"}},
    )
    body = r.json()
    assert body["pending_lookups"] == []
    artist_op = next(
        o for p in body["plans"] for o in p["ops"] if o["field"] == "artist"
    )
    assert artist_op["confidence"] == "high" and artist_op["verified"]


def test_verify_failures_are_retried_not_cached(auth_client: TestClient, monkeypatch):
    def boom(name: str) -> tuple[int, int]:
        raise OSError("network down")

    monkeypatch.setattr("app.library.cleanup_lookup.fetch_name_scores", boom)
    r = auth_client.post(
        "/api/library/cleanup/verify", json={"names": ["Flaky Lookup Name"]}
    )
    assert r.json() == {"verified": 0, "failed": ["Flaky Lookup Name"]}

    monkeypatch.setattr(
        "app.library.cleanup_lookup.fetch_name_scores", lambda name: (95, 10)
    )
    r = auth_client.post(
        "/api/library/cleanup/verify", json={"names": ["Flaky Lookup Name"]}
    )
    assert r.json()["verified"] == 1  # not poisoned by the earlier failure


def test_revert_from_uploaded_journal(auth_client: TestClient):
    _seed_folder("CleanupJournal", ["06 - Foxtrot.wav"])
    r = auth_client.post(
        "/api/library/cleanup/analyze",
        json={"scope": {"type": "folder", "path": "CleanupJournal"}},
    )
    ops = [o for o in _ops_in(r.json()["plans"]) if o["kind"] == "rename"]
    r = auth_client.post("/api/library/cleanup/apply", json={"ops": ops})
    batch_id = r.json()["batch_id"]
    items = auth_client.get(f"/api/library/cleanup/batches/{batch_id}").json()["items"]

    # Revert via the journal payload (disaster path — no batch row needed).
    r = auth_client.post("/api/library/cleanup/revert", json={"items": items})
    assert r.status_code == 200, r.text
    assert r.json()["reverted"] == len(items)
    music_dir = Path(os.environ["MUSIC_DIR"])
    assert (music_dir / "CleanupJournal" / "06 - Foxtrot.wav").is_file()
