"""Deterministic, path-free D&D tag evidence from descriptive metadata.

This module finds explicit controlled-vocabulary terms and conservative aliases.
It does not decide or persist tags. Callers can disclose the matches as hypotheses
for a model or present them directly for human review.
"""

from __future__ import annotations

import re
import unicodedata
from collections.abc import Mapping
from dataclasses import dataclass
from typing import Literal

from app.assistant.tags import DND_STARTER_TAG_GROUPS

_WORD_RE = re.compile(r"[a-z0-9]+")


MetadataField = Literal["title", "artist", "album", "origin", "genre"]


@dataclass(frozen=True)
class MetadataTagMatch:
    tag: str
    matched_fields: tuple[MetadataField, ...]
    matched_terms: tuple[str, ...]


# Aliases deliberately require semantic metadata. Signal-only properties such as
# tempo or energy are handled separately and must not create setting/scene tags.
_TAG_ALIASES: dict[str, tuple[str, ...]] = {
    "medieval": ("medieval", "middle ages"),
    "tavern": ("tavern", "inn", "pub", "alehouse"),
    "dungeon": ("dungeon", "crypt", "catacomb", "catacombs"),
    "castle": ("castle", "palace", "citadel"),
    "village": ("village", "hamlet"),
    "forest": ("forest", "woodland", "woods"),
    "wilderness": ("wilderness", "wilds", "green hills"),
    "temple": ("temple", "chapel", "shrine"),
    "ruins": ("ruin", "ruins", "ruined"),
    "seafaring": ("seafaring", "sea", "ocean", "sailor", "sails", "fleet", "naval", "maritime"),
    "dancing": ("dance", "dances", "dancing", "jig", "reel", "waltz"),
    "feast": ("feast", "banquet"),
    "travel": ("travel", "journey", "journeys", "road", "wayfarer", "voyage"),
    "exploration": ("exploration", "adventure", "expedition"),
    "combat": ("combat", "battle", "battles", "fight", "war"),
    "stealth": ("stealth", "sneaking", "covert"),
    "investigation": ("investigation", "detective", "inquiry", "clue"),
    "rest": ("rest", "lullaby", "sleep", "repose"),
    "festive": (
        "festive",
        "festival",
        "celebration",
        "celebratory",
        "party",
        "dance",
        "dances",
        "dancing",
        "jig",
    ),
    "heroic": ("heroic", "hero", "triumph", "triumphant", "crown guard"),
    "mysterious": ("mysterious", "mystery", "enigmatic"),
    "tense": ("tense", "tension", "suspense", "danger", "dangerous"),
    "dark": ("dark", "dread", "sinister", "horror"),
    "calm": ("calm", "peaceful", "gentle", "quiet", "lullaby"),
    "eerie": ("eerie", "haunting", "ghost", "whispers", "crypt", "catacomb", "catacombs"),
    "melancholy": ("melancholy", "melancholic", "sad", "sorrow", "tragic"),
    "romantic": ("romantic", "romance", "love"),
}

_STARTER_TAGS = frozenset(tag for group in DND_STARTER_TAG_GROUPS for tag in group.tags)
if frozenset(_TAG_ALIASES) != _STARTER_TAGS:
    raise RuntimeError("metadata tag aliases must cover the D&D starter vocabulary")


def _tokens(value: str) -> frozenset[str]:
    normalized = unicodedata.normalize("NFKD", value.casefold())
    ascii_value = "".join(
        character for character in normalized if not unicodedata.combining(character)
    )
    return frozenset(_WORD_RE.findall(ascii_value))


def infer_metadata_tag_matches(
    fields: Mapping[MetadataField, str],
) -> tuple[MetadataTagMatch, ...]:
    """Return deterministic tag hypotheses with compact field-level provenance."""

    field_tokens = {field: _tokens(value) for field, value in fields.items() if value.strip()}
    matches: list[MetadataTagMatch] = []
    for tag in (tag for group in DND_STARTER_TAG_GROUPS for tag in group.tags):
        matched_fields: set[MetadataField] = set()
        matched_terms: set[str] = set()
        for alias in _TAG_ALIASES[tag]:
            alias_tokens = _tokens(alias)
            for field, tokens in field_tokens.items():
                if alias_tokens <= tokens:
                    matched_fields.add(field)
                    matched_terms.add(alias)
        if matched_fields:
            matches.append(
                MetadataTagMatch(
                    tag=tag,
                    matched_fields=tuple(sorted(matched_fields)),
                    matched_terms=tuple(sorted(matched_terms)),
                )
            )
    return tuple(matches)
