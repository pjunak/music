"""Deterministic controlled-tag evidence from disclosed library context.

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

_WORD_RE = re.compile(r"[a-z0-9]+")


MetadataField = Literal[
    "title",
    "artist",
    "album",
    "origin",
    "genre",
    "library_path",
]


@dataclass(frozen=True)
class MetadataTagMatch:
    tag: str
    matched_fields: tuple[MetadataField, ...]
    matched_terms: tuple[str, ...]


def _tokens(value: str) -> frozenset[str]:
    normalized = unicodedata.normalize("NFKD", value.casefold())
    ascii_value = "".join(
        character for character in normalized if not unicodedata.combining(character)
    )
    return frozenset(_WORD_RE.findall(ascii_value))


def infer_metadata_matches_for_aliases(
    fields: Mapping[MetadataField, str],
    aliases_by_tag: Mapping[str, tuple[str, ...]],
) -> tuple[MetadataTagMatch, ...]:
    """Match an operator vocabulary's exact names and aliases with provenance."""

    field_tokens = {field: _tokens(value) for field, value in fields.items() if value.strip()}
    matches: list[MetadataTagMatch] = []
    for tag, aliases in aliases_by_tag.items():
        matched_fields: set[MetadataField] = set()
        matched_terms: set[str] = set()
        for alias in aliases:
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
