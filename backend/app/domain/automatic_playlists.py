"""Versioned local rules that materialize into ordinary playlist items."""

from __future__ import annotations

import hashlib
import json
from collections.abc import Mapping
from dataclasses import dataclass
from typing import Literal

from pydantic import BaseModel, ConfigDict, Field, field_validator, model_validator
from sqlalchemy import delete, select
from sqlalchemy.orm import Session

from app.assistant.analysis import load_current_metadata_profiles
from app.assistant.engine import TrackAnalysisProfile
from app.assistant.tags import load_manual_tags, normalize_manual_tags
from app.models.base import utcnow
from app.models.playlist import Playlist, PlaylistItem
from app.models.track import Track


class AutomaticPlaylistError(RuntimeError):
    def __init__(self, code: str, message: str) -> None:
        super().__init__(message)
        self.code = code
        self.message = message


class AutomaticPlaylistRuleV1(BaseModel):
    model_config = ConfigDict(extra="forbid", frozen=True)

    schema_version: Literal["automatic-playlist/v1"] = Field(alias="schema")
    include_tags: list[str] = Field(default_factory=list, max_length=32)
    match: Literal["any", "all"] = "any"
    exclude_tags: list[str] = Field(default_factory=list, max_length=32)
    tag_sources: Literal["manual", "manual_and_local"] = "manual"
    min_bpm: int | None = Field(default=None, ge=1, le=999)
    max_bpm: int | None = Field(default=None, ge=1, le=999)
    include_unknown_bpm: bool = True
    maximum_tracks: int = Field(default=200, ge=1, le=1000)
    order_by: Literal[
        "title",
        "newest",
        "bpm_ascending",
        "bpm_descending",
    ] = "title"

    @field_validator("include_tags", "exclude_tags")
    @classmethod
    def normalize_tags(cls, values: list[str]) -> list[str]:
        return list(normalize_manual_tags(values))

    @model_validator(mode="after")
    def valid_rule(self) -> AutomaticPlaylistRuleV1:
        if set(self.include_tags) & set(self.exclude_tags):
            raise ValueError("included and excluded tags must be disjoint")
        if (
            self.min_bpm is not None
            and self.max_bpm is not None
            and self.min_bpm > self.max_bpm
        ):
            raise ValueError("min_bpm cannot be greater than max_bpm")
        return self


@dataclass(frozen=True)
class AutomaticPlaylistResolution:
    source_signature: str
    library_tracks: int
    tracks: tuple[Track, ...]


def parse_automatic_rule(value: str) -> AutomaticPlaylistRuleV1:
    try:
        return AutomaticPlaylistRuleV1.model_validate_json(value)
    except ValueError as exc:
        raise AutomaticPlaylistError(
            "automatic_rule_invalid",
            "The stored automatic playlist rule is invalid. Edit or disable it.",
        ) from exc


def _effective_tags(
    track_id: int,
    manual: Mapping[int, tuple[str, ...]],
    local_profiles: Mapping[int, TrackAnalysisProfile],
    *,
    include_local: bool,
) -> set[str]:
    tags = set(manual.get(track_id, ()))
    if include_local:
        profile = local_profiles.get(track_id)
        profile_moods = getattr(profile, "moods", ()) if profile is not None else ()
        tags.update(profile_moods)
    return tags


def _matches(
    track: Track,
    tags: set[str],
    rule: AutomaticPlaylistRuleV1,
) -> bool:
    included = set(rule.include_tags)
    if included:
        if rule.match == "all" and not included.issubset(tags):
            return False
        if rule.match == "any" and included.isdisjoint(tags):
            return False
    if set(rule.exclude_tags) & tags:
        return False
    if track.bpm is None:
        return rule.include_unknown_bpm
    if rule.min_bpm is not None and track.bpm < rule.min_bpm:
        return False
    return rule.max_bpm is None or track.bpm <= rule.max_bpm


def _sort_key(track: Track, rule: AutomaticPlaylistRuleV1) -> tuple[object, ...]:
    title = (track.display_title or track.title or track.path).casefold()
    if rule.order_by == "newest":
        return (-track.added_at.timestamp(), title, track.id)
    if rule.order_by == "bpm_ascending":
        return (track.bpm is None, track.bpm or 0, title, track.id)
    if rule.order_by == "bpm_descending":
        return (track.bpm is None, -(track.bpm or 0), title, track.id)
    return (title, track.id)


def resolve_automatic_playlist(
    db: Session,
    rule: AutomaticPlaylistRuleV1,
) -> AutomaticPlaylistResolution:
    tracks = list(db.scalars(select(Track).order_by(Track.id)).all())
    manual_mapping = load_manual_tags(db, [track.id for track in tracks])
    manual = {track_id: tuple(tags) for track_id, tags in manual_mapping.items()}
    include_local = rule.tag_sources == "manual_and_local"
    local_profiles: Mapping[int, TrackAnalysisProfile] = (
        load_current_metadata_profiles(db, tracks) if include_local else {}
    )
    effective_tags = {
        track.id: _effective_tags(
            track.id,
            manual,
            local_profiles,
            include_local=include_local,
        )
        for track in tracks
    }
    signature_payload = {
        "rule": rule.model_dump(mode="json", by_alias=True),
        "tracks": [
            {
                "id": track.id,
                "path": track.path,
                "mtime": track.mtime,
                "size_bytes": track.size_bytes,
                "bpm": track.bpm,
                "display_title": track.display_title,
                "title": track.title,
                "tags": sorted(effective_tags[track.id]),
            }
            for track in tracks
        ],
    }
    source_signature = hashlib.sha256(
        json.dumps(
            signature_payload,
            ensure_ascii=False,
            sort_keys=True,
            separators=(",", ":"),
        ).encode("utf-8")
    ).hexdigest()
    matched = [
        track for track in tracks if _matches(track, effective_tags[track.id], rule)
    ]
    matched.sort(key=lambda track: _sort_key(track, rule))
    return AutomaticPlaylistResolution(
        source_signature=source_signature,
        library_tracks=len(tracks),
        tracks=tuple(matched[: rule.maximum_tracks]),
    )


def materialize_automatic_playlist(
    db: Session,
    playlist: Playlist,
    rule: AutomaticPlaylistRuleV1,
    resolution: AutomaticPlaylistResolution,
) -> None:
    db.execute(delete(PlaylistItem).where(PlaylistItem.playlist_id == playlist.id))
    db.add_all(
        PlaylistItem(
            playlist_id=playlist.id,
            position=position,
            track_id=track.id,
        )
        for position, track in enumerate(resolution.tracks)
    )
    playlist.automatic_rule_json = rule.model_dump_json(by_alias=True)
    playlist.automatic_source_signature = resolution.source_signature
    playlist.automatic_refreshed_at = utcnow()
    playlist.updated_at = utcnow()
    db.commit()


def refresh_automatic_playlist_if_stale(
    db: Session,
    playlist: Playlist,
) -> bool:
    if not playlist.automatic_rule_json:
        return False
    rule = parse_automatic_rule(playlist.automatic_rule_json)
    resolution = resolve_automatic_playlist(db, rule)
    if playlist.automatic_source_signature == resolution.source_signature:
        return False
    materialize_automatic_playlist(db, playlist, rule, resolution)
    return True


def disable_automatic_playlist(db: Session, playlist: Playlist) -> None:
    playlist.automatic_rule_json = ""
    playlist.automatic_source_signature = None
    playlist.automatic_refreshed_at = None
    playlist.updated_at = utcnow()
    db.commit()
