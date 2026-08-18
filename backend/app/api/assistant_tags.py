"""Operator-owned playlist tags, kept separate from generated analysis."""

from typing import Literal

from fastapi import APIRouter, HTTPException, Query
from sqlalchemy import func, or_, select

from app.api.deps import CurrentUser, DbSession
from app.assistant.analysis import (
    LOCAL_METADATA_ANALYZER_ID,
    load_current_metadata_profiles,
)
from app.assistant.tag_schemas import (
    LibraryTagPage,
    LibraryTagTrack,
    ManualTagCatalog,
    ManualTagPatch,
    StarterTagGroupOut,
)
from app.assistant.tags import (
    DND_STARTER_TAG_GROUPS,
    TagLimitError,
    load_manual_tags,
    normalize_manual_tag,
    patch_manual_tags,
    used_manual_tags,
)
from app.models.track import Track
from app.models.track_user_tag import TrackUserTag

router = APIRouter(prefix="/api/assistant/library-tags", tags=["assistant-tags"])


def _track_out(
    track: Track,
    manual_tags: list[str],
    *,
    analysis_tags: list[str],
    analysis_confidence: Literal["high", "medium", "low"] | None,
) -> LibraryTagTrack:
    return LibraryTagTrack(
        track_id=track.id,
        path=track.path,
        title=track.title,
        display_title=track.display_title,
        artist=track.artist,
        album=track.album,
        manual_tags=manual_tags,
        analysis_analyzer=(
            LOCAL_METADATA_ANALYZER_ID if analysis_confidence is not None else None
        ),
        analysis_tags=analysis_tags,
        analysis_confidence=analysis_confidence,
    )


@router.get("/catalog", response_model=ManualTagCatalog)
def tag_catalog(_user: CurrentUser, db: DbSession) -> ManualTagCatalog:
    return ManualTagCatalog(
        starter_groups=[
            StarterTagGroupOut(
                key=group.key,
                label=group.label,
                tags=list(group.tags),
            )
            for group in DND_STARTER_TAG_GROUPS
        ],
        used_tags=list(used_manual_tags(db)),
    )


@router.get("", response_model=LibraryTagPage)
def list_library_tags(
    _user: CurrentUser,
    db: DbSession,
    search: str = Query(default="", max_length=128),
    tag: str | None = Query(default=None, max_length=64),
    offset: int = Query(default=0, ge=0),
    limit: int = Query(default=50, ge=1, le=100),
) -> LibraryTagPage:
    filters = []
    search = search.strip()
    if search:
        filters.append(
            or_(
                Track.display_title.contains(search, autoescape=True),
                Track.title.contains(search, autoescape=True),
                Track.artist.contains(search, autoescape=True),
                Track.album.contains(search, autoescape=True),
                Track.path.contains(search, autoescape=True),
            )
        )

    normalized_tag: str | None = None
    if tag is not None:
        try:
            normalized_tag = normalize_manual_tag(tag)
        except ValueError as exc:
            raise HTTPException(status_code=422, detail=str(exc)) from exc

    query = select(Track)
    count_query = select(func.count()).select_from(Track)
    if normalized_tag is not None:
        query = query.join(TrackUserTag).where(TrackUserTag.tag == normalized_tag)
        count_query = count_query.join(TrackUserTag).where(
            TrackUserTag.tag == normalized_tag
        )
    if filters:
        query = query.where(*filters)
        count_query = count_query.where(*filters)

    total = int(db.scalar(count_query) or 0)
    tracks = list(
        db.scalars(
            query.order_by(
                func.lower(
                    func.coalesce(func.nullif(Track.display_title, ""), Track.title)
                ),
                Track.id,
            )
            .offset(offset)
            .limit(limit)
        ).all()
    )
    track_ids = [track.id for track in tracks]
    manual_by_track = load_manual_tags(db, track_ids)
    profiles = load_current_metadata_profiles(db, tracks)
    items: list[LibraryTagTrack] = []
    for track in tracks:
        profile = profiles.get(track.id)
        items.append(
            _track_out(
                track,
                list(manual_by_track.get(track.id, ())),
                analysis_tags=list(profile.moods) if profile is not None else [],
                analysis_confidence=(profile.confidence if profile is not None else None),
            )
        )
    return LibraryTagPage(items=items, total=total, offset=offset, limit=limit)


@router.patch("/{track_id}", response_model=LibraryTagTrack)
def update_library_tags(
    track_id: int,
    payload: ManualTagPatch,
    _user: CurrentUser,
    db: DbSession,
) -> LibraryTagTrack:
    track = db.get(Track, track_id)
    if track is None:
        raise HTTPException(status_code=404, detail="Track not found")
    try:
        manual_tags = patch_manual_tags(
            db,
            track_id,
            add=payload.add,
            remove=payload.remove,
        )
    except (TagLimitError, ValueError) as exc:
        raise HTTPException(status_code=422, detail=str(exc)) from exc
    profile = load_current_metadata_profiles(db, [track]).get(track.id)
    return _track_out(
        track,
        list(manual_tags),
        analysis_tags=list(profile.moods) if profile is not None else [],
        analysis_confidence=profile.confidence if profile is not None else None,
    )
