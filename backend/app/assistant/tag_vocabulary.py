"""Persistent, operator-editable controlled vocabulary for Assistant tagging."""

from __future__ import annotations

import hashlib
import json
from dataclasses import dataclass
from typing import Any, Literal, cast

from pydantic import BaseModel, ConfigDict, Field, field_validator, model_validator
from sqlalchemy import CursorResult, update
from sqlalchemy.orm import Session

from app.assistant.tags import normalize_manual_tag
from app.models.assistant_tag_vocabulary import AssistantTagVocabulary
from app.models.base import utcnow

TAG_VOCABULARY_KEY = "library"
TAG_VOCABULARY_SCHEMA: Literal["assistant-tag-vocabulary/v1"] = (
    "assistant-tag-vocabulary/v1"
)
MAX_VOCABULARY_TAGS = 200


class _StrictVocabularyModel(BaseModel):
    model_config = ConfigDict(extra="forbid", frozen=True)


class TagVocabularyEntry(_StrictVocabularyModel):
    id: str = Field(pattern=r"^[a-z0-9][a-z0-9._-]{1,63}$")
    name: str = Field(min_length=1, max_length=64)
    description: str = Field(min_length=2, max_length=300)
    aliases: list[str] = Field(default_factory=list, max_length=24)

    @field_validator("name")
    @classmethod
    def normalize_name(cls, value: str) -> str:
        return normalize_manual_tag(value)

    @field_validator("description")
    @classmethod
    def normalize_description(cls, value: str) -> str:
        normalized = " ".join(value.split())
        if len(normalized) < 2:
            raise ValueError("a tag description must contain at least two characters")
        return normalized

    @field_validator("aliases")
    @classmethod
    def normalize_aliases(cls, value: list[str]) -> list[str]:
        return list(dict.fromkeys(normalize_manual_tag(alias) for alias in value))

    @model_validator(mode="after")
    def aliases_do_not_repeat_name(self) -> TagVocabularyEntry:
        if self.name in self.aliases:
            raise ValueError("a tag name cannot also be one of its aliases")
        return self


class TagVocabularyGroup(_StrictVocabularyModel):
    key: str = Field(pattern=r"^[a-z0-9][a-z0-9_-]{1,31}$")
    label: str = Field(min_length=1, max_length=64)
    description: str = Field(default="", max_length=300)
    tags: list[TagVocabularyEntry] = Field(default_factory=list, max_length=100)

    @field_validator("label")
    @classmethod
    def normalize_label(cls, value: str) -> str:
        normalized = " ".join(value.split())
        if not normalized:
            raise ValueError("a vocabulary group label cannot be blank")
        return normalized

    @field_validator("description")
    @classmethod
    def normalize_group_description(cls, value: str) -> str:
        return " ".join(value.split())


class TagVocabularyDocument(_StrictVocabularyModel):
    schema_version: Literal["assistant-tag-vocabulary/v1"] = TAG_VOCABULARY_SCHEMA
    groups: list[TagVocabularyGroup] = Field(min_length=1, max_length=20)

    @model_validator(mode="after")
    def unique_vocabulary(self) -> TagVocabularyDocument:
        group_keys = [group.key for group in self.groups]
        if len(group_keys) != len(set(group_keys)):
            raise ValueError("vocabulary group keys must be unique")

        entries = [tag for group in self.groups for tag in group.tags]
        if not entries:
            raise ValueError("the vocabulary must contain at least one tag")
        if len(entries) > MAX_VOCABULARY_TAGS:
            raise ValueError(
                f"the vocabulary cannot contain more than {MAX_VOCABULARY_TAGS} tags"
            )
        ids = [tag.id for tag in entries]
        names = [tag.name for tag in entries]
        if len(ids) != len(set(ids)):
            raise ValueError("vocabulary tag IDs must be unique")
        if len(names) != len(set(names)):
            raise ValueError("vocabulary tag names must be unique")

        canonical_names = set(names)
        alias_owners: dict[str, str] = {}
        for tag in entries:
            for alias in tag.aliases:
                if alias in canonical_names:
                    raise ValueError(
                        f"alias '{alias}' conflicts with a canonical tag name"
                    )
                owner = alias_owners.get(alias)
                if owner is not None:
                    raise ValueError(
                        f"alias '{alias}' belongs to both '{owner}' and '{tag.name}'"
                    )
                alias_owners[alias] = tag.name
        return self


@dataclass(frozen=True)
class TagVocabularySnapshot:
    document: TagVocabularyDocument
    revision: int
    fingerprint: str

    @property
    def entries(self) -> tuple[TagVocabularyEntry, ...]:
        return tuple(tag for group in self.document.groups for tag in group.tags)

    @property
    def ids(self) -> frozenset[str]:
        return frozenset(tag.id for tag in self.entries)

    @property
    def names(self) -> frozenset[str]:
        return frozenset(tag.name for tag in self.entries)

    @property
    def by_id(self) -> dict[str, TagVocabularyEntry]:
        return {tag.id: tag for tag in self.entries}

    @property
    def by_name(self) -> dict[str, TagVocabularyEntry]:
        return {tag.name: tag for tag in self.entries}

    @property
    def aliases(self) -> dict[str, TagVocabularyEntry]:
        return {alias: tag for tag in self.entries for alias in tag.aliases}

    @property
    def group_by_tag_id(self) -> dict[str, TagVocabularyGroup]:
        return {
            tag.id: group
            for group in self.document.groups
            for tag in group.tags
        }


class TagVocabularyConflictError(ValueError):
    pass


def _entry(
    group: str,
    name: str,
    description: str,
    *aliases: str,
) -> TagVocabularyEntry:
    return TagVocabularyEntry(
        id=f"{group}.{name.replace(' ', '-')}",
        name=name,
        description=description,
        aliases=list(aliases),
    )


def default_tag_vocabulary() -> TagVocabularyDocument:
    return TagVocabularyDocument(
        groups=[
            TagVocabularyGroup(
                key="setting",
                label="Setting",
                description="Where the scene takes place or the culture it evokes.",
                tags=[
                    _entry("setting", "medieval", "Pre-modern European courtly, folk, or feudal atmosphere.", "middle ages"),
                    _entry("setting", "tavern", "Inn, alehouse, common-room, or drinking-house setting.", "inn", "pub", "alehouse"),
                    _entry("setting", "dungeon", "Enclosed underground danger, prison, crypt, or hostile delve."),
                    _entry("setting", "castle", "Fortified keep, royal stronghold, battlement, or great hall."),
                    _entry("setting", "village", "Small inhabited rural settlement or homestead community."),
                    _entry("setting", "forest", "Woodland, grove, canopy, or tree-dense natural setting."),
                    _entry("setting", "wilderness", "Remote uncultivated land beyond settlements and roads."),
                    _entry("setting", "temple", "Sacred place, shrine, chapel, or organized religious setting."),
                    _entry("setting", "ruins", "Abandoned, broken, or ancient constructed remains.", "ruin"),
                    _entry("setting", "seafaring", "Ships, sailors, ocean travel, ports, or life at sea.", "ocean voyage", "nautical"),
                ],
            ),
            TagVocabularyGroup(
                key="scene",
                label="Scene",
                description="What the players or characters are doing.",
                tags=[
                    _entry("scene", "dancing", "Rhythmic social, folk, courtly, or celebratory dance."),
                    _entry("scene", "feast", "Banquet, communal meal, revel, or abundant celebration."),
                    _entry("scene", "travel", "A journey, road sequence, voyage, or movement between places.", "journey"),
                    _entry("scene", "exploration", "Discovery, surveying, delving, or cautiously entering the unknown."),
                    _entry("scene", "combat", "Active battle, confrontation, attack, or martial conflict.", "battle"),
                    _entry("scene", "stealth", "Sneaking, infiltration, hiding, or avoiding detection."),
                    _entry("scene", "investigation", "Searching for clues, deduction, inquiry, or detective work.", "detective work"),
                    _entry(
                        "scene",
                        "rest",
                        "Recovery, sleep, camp, respite, or a safe pause.",
                        "repose",
                        "quiet sleep",
                    ),
                ],
            ),
            TagVocabularyGroup(
                key="mood",
                label="Mood",
                description="The emotional tone the music supports.",
                tags=[
                    _entry("mood", "festive", "Joyful public celebration, revelry, or cheerful ceremony."),
                    _entry("mood", "heroic", "Courageous, triumphant, noble, or larger-than-life resolve."),
                    _entry("mood", "mysterious", "Uncertain, secretive, curious, or unexplained atmosphere."),
                    _entry("mood", "tense", "Pressure, suspense, urgency, or expectation of danger."),
                    _entry("mood", "dark", "Bleak, threatening, morally shadowed, or oppressive tone."),
                    _entry("mood", "calm", "Peaceful, settled, gentle, or emotionally untroubled tone."),
                    _entry("mood", "eerie", "Uncanny, ghostly, strange, or quietly unsettling tone."),
                    _entry("mood", "melancholy", "Wistful sadness, loss, reflection, or subdued grief.", "sad"),
                    _entry(
                        "mood",
                        "romantic",
                        "Tenderness, intimacy, affection, or love-associated warmth.",
                        "romance",
                        "love theme",
                    ),
                ],
            ),
        ]
    )


def _document_json(document: TagVocabularyDocument) -> str:
    return json.dumps(
        document.model_dump(mode="json"),
        ensure_ascii=False,
        separators=(",", ":"),
        sort_keys=True,
    )


def vocabulary_fingerprint(document: TagVocabularyDocument) -> str:
    return hashlib.sha256(_document_json(document).encode("utf-8")).hexdigest()


def default_tag_vocabulary_snapshot() -> TagVocabularySnapshot:
    document = default_tag_vocabulary()
    return TagVocabularySnapshot(
        document=document,
        revision=1,
        fingerprint=vocabulary_fingerprint(document),
    )


def load_tag_vocabulary(db: Session) -> TagVocabularySnapshot:
    row = db.get(AssistantTagVocabulary, TAG_VOCABULARY_KEY)
    if row is None:
        document = default_tag_vocabulary()
        row = AssistantTagVocabulary(
            key=TAG_VOCABULARY_KEY,
            revision=1,
            document_json=_document_json(document),
        )
        db.add(row)
        db.commit()
        db.refresh(row)
    document = TagVocabularyDocument.model_validate_json(row.document_json)
    return TagVocabularySnapshot(
        document=document,
        revision=row.revision,
        fingerprint=vocabulary_fingerprint(document),
    )


def replace_tag_vocabulary(
    db: Session,
    *,
    expected_revision: int,
    document: TagVocabularyDocument,
) -> TagVocabularySnapshot:
    current = load_tag_vocabulary(db)
    if current.revision != expected_revision:
        raise TagVocabularyConflictError(
            "The tag vocabulary changed after this page was loaded. Reload it and try again."
        )
    result = cast(
        "CursorResult[Any]",
        db.execute(
            update(AssistantTagVocabulary)
            .where(
                AssistantTagVocabulary.key == TAG_VOCABULARY_KEY,
                AssistantTagVocabulary.revision == expected_revision,
            )
            .values(
                document_json=_document_json(document),
                revision=expected_revision + 1,
                updated_at=utcnow(),
            )
        ),
    )
    if result.rowcount != 1:
        db.rollback()
        raise TagVocabularyConflictError(
            "The tag vocabulary changed while it was being saved. Reload it and try again."
        )
    db.commit()
    return TagVocabularySnapshot(
        document=document,
        revision=expected_revision + 1,
        fingerprint=vocabulary_fingerprint(document),
    )
