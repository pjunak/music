"""Strict provider contract and synthetic evaluation for metadata music tagging."""

from __future__ import annotations

import re
from collections.abc import Callable, Sequence
from copy import deepcopy
from pathlib import Path
from typing import Annotated, Literal

from pydantic import BaseModel, ConfigDict, Field, ValidationError, model_validator

from app.assistant.local import analyze_track_metadata
from app.assistant.metadata_tag_evidence import (
    MetadataField,
    infer_metadata_matches_for_aliases,
    infer_metadata_tag_matches,
)
from app.assistant.providers.execution import StructuredModelRequest, StructuredModelResult
from app.assistant.schema_diagnostics import safe_validation_diagnostic
from app.assistant.structured_harness import (
    StructuredTaskDefinition,
    build_structured_request,
    numbered_rules,
)
from app.assistant.tag_vocabulary import (
    TagVocabularySnapshot,
    default_tag_vocabulary_snapshot,
)

MODEL_TAGGER_INPUT_CONTRACT: Literal["assistant-music-tagger-input/v4"] = (
    "assistant-music-tagger-input/v4"
)
MODEL_TAGGER_OUTPUT_CONTRACT: Literal["assistant-music-tagger-output/v2"] = (
    "assistant-music-tagger-output/v2"
)
MODEL_TAGGING_EVALUATION_CONTRACT: Literal[
    "assistant-music-tagger-evaluation/v3"
] = "assistant-music-tagger-evaluation/v3"
MODEL_TAG_ANALYZER_ID: Literal["model-evidence-tagger/v4"] = (
    "model-evidence-tagger/v4"
)
MODEL_TAG_BATCH_SIZE = 20
MAX_MODEL_TAGS_PER_TRACK = 8
_MAX_MODEL_OUTPUT_TOKENS = 8_000
_SAFE_ERROR_CODE = re.compile(r"^[a-z0-9_]{1,64}$")

_DEFAULT_VOCABULARY = default_tag_vocabulary_snapshot()
MODEL_TAG_VOCABULARY: tuple[str, ...] = tuple(
    tag.name for tag in _DEFAULT_VOCABULARY.entries
)
_MODEL_TAG_SET = frozenset(MODEL_TAG_VOCABULARY)

BoundedText = Annotated[str, Field(max_length=512)]
BoundedEvidence = Annotated[str, Field(min_length=1, max_length=512)]
TagConfidence = Literal["high", "medium", "low"]


def _all_tag_confidences() -> list[TagConfidence]:
    return ["high", "medium", "low"]


class _StrictModel(BaseModel):
    model_config = ConfigDict(extra="forbid", frozen=True)


class ModelTagAudioEvidence(_StrictModel):
    analyzer_id: Literal["local-audio/v1"]
    energy: float = Field(ge=0.0, le=1.0)
    brightness: float = Field(ge=0.0, le=1.0)
    tension: float = Field(ge=0.0, le=1.0)
    tempo_bpm: float | None = Field(default=None, gt=0.0, le=999.0)
    activity: float | None = Field(default=None, ge=0.0, le=1.0)
    dynamic_range: float | None = Field(default=None, ge=0.0, le=1.0)
    rhythmic_density: float | None = Field(default=None, ge=0.0, le=1.0)
    rhythmic_stability: float | None = Field(default=None, ge=0.0, le=1.0)
    confidence: TagConfidence


class ModelTagMetadataMatch(_StrictModel):
    tag_id: str = Field(min_length=2, max_length=64)
    matched_fields: list[
        Literal["title", "artist", "album", "origin", "genre"]
    ] = Field(min_length=1, max_length=5)
    matched_terms: list[Annotated[str, Field(min_length=1, max_length=64)]] = Field(
        min_length=1,
        max_length=8,
    )


class ModelTagMetadataEvidence(_StrictModel):
    analyzer_id: Literal["local-metadata-evidence/v1"]
    canonical_title_source: Literal["display_title", "title", "none"]
    candidate_tag_ids: list[str] = Field(max_length=32)
    tag_matches: list[ModelTagMetadataMatch] = Field(max_length=32)
    energy: float = Field(ge=0.0, le=1.0)
    brightness: float = Field(ge=0.0, le=1.0)
    tension: float = Field(ge=0.0, le=1.0)
    confidence: TagConfidence

    @model_validator(mode="after")
    def consistent_controlled_tag_matches(self) -> ModelTagMetadataEvidence:
        matched_ids = [match.tag_id for match in self.tag_matches]
        if len(matched_ids) != len(set(matched_ids)):
            raise ValueError("metadata evidence tag matches must be unique")
        if self.candidate_tag_ids != matched_ids:
            raise ValueError("metadata evidence candidates must match tag provenance")
        return self


class ModelTagTrackInput(_StrictModel):
    track_id: int = Field(gt=0)
    title: BoundedText
    display_title: BoundedText
    artist: BoundedText
    album: BoundedText
    origin: BoundedText
    genre: str = Field(max_length=128)
    length_s: float = Field(ge=0.0)
    bpm: int | None = Field(default=None, ge=1, le=999)
    metadata_evidence: ModelTagMetadataEvidence | None = None
    audio_evidence: ModelTagAudioEvidence | None = None


class ModelTaggerInput(_StrictModel):
    schema_version: Literal["assistant-music-tagger-input/v4"]
    vocabulary: list[ModelTagDefinition] = Field(min_length=1, max_length=200)
    tracks: list[ModelTagTrackInput] = Field(min_length=1, max_length=20)


class ModelTagDefinition(_StrictModel):
    tag_id: str = Field(min_length=2, max_length=64)
    name: str = Field(min_length=1, max_length=64)
    group: str = Field(min_length=1, max_length=64)
    description: str = Field(min_length=2, max_length=300)
    aliases: list[str] = Field(default_factory=list, max_length=24)


class ModelTagTrackChoice(_StrictModel):
    model_config = ConfigDict(extra="forbid", frozen=True, strict=True)

    track_id: int = Field(gt=0)
    tag_ids: list[str] = Field(max_length=MAX_MODEL_TAGS_PER_TRACK)
    energy: float = Field(ge=0.0, le=1.0)
    brightness: float = Field(ge=0.0, le=1.0)
    tension: float = Field(ge=0.0, le=1.0)
    confidence: TagConfidence
    evidence: list[BoundedEvidence] = Field(max_length=4)

    @model_validator(mode="after")
    def unique_tags(self) -> ModelTagTrackChoice:
        if len(set(self.tag_ids)) != len(self.tag_ids):
            raise ValueError("tag_ids must be unique")
        return self


class ModelTaggerOutput(_StrictModel):
    model_config = ConfigDict(extra="forbid", frozen=True, strict=True)

    schema_version: Literal["assistant-music-tagger-output/v2"]
    tracks: list[ModelTagTrackChoice] = Field(min_length=1, max_length=20)

    @model_validator(mode="after")
    def unique_tracks(self) -> ModelTaggerOutput:
        ids = [track.track_id for track in self.tracks]
        if len(ids) != len(set(ids)):
            raise ValueError("track IDs must be unique")
        return self


class ModelTagTrackOutput(_StrictModel):
    track_id: int = Field(gt=0)
    tags: list[str] = Field(max_length=MAX_MODEL_TAGS_PER_TRACK)
    energy: float = Field(ge=0.0, le=1.0)
    brightness: float = Field(ge=0.0, le=1.0)
    tension: float = Field(ge=0.0, le=1.0)
    confidence: TagConfidence
    evidence: list[BoundedEvidence] = Field(max_length=4)


class TagQualityCase(_StrictModel):
    id: str = Field(min_length=1, max_length=128)
    description: str = Field(min_length=1, max_length=512)
    track: ModelTagTrackInput
    required_tags: list[str] = Field(max_length=MAX_MODEL_TAGS_PER_TRACK)
    forbidden_tags: list[str] = Field(max_length=MAX_MODEL_TAGS_PER_TRACK)
    allowed_confidences: list[TagConfidence] = Field(
        default_factory=_all_tag_confidences,
        min_length=1,
        max_length=3,
    )
    minimum_evidence_items: int = Field(default=0, ge=0, le=4)

    @model_validator(mode="after")
    def valid_expectations(self) -> TagQualityCase:
        expected = set(self.required_tags) | set(self.forbidden_tags)
        if not expected <= _MODEL_TAG_SET:
            raise ValueError("evaluation tags must use the controlled vocabulary")
        if len(set(self.required_tags)) != len(self.required_tags):
            raise ValueError("required tags must be unique")
        if len(set(self.forbidden_tags)) != len(self.forbidden_tags):
            raise ValueError("forbidden tags must be unique")
        if set(self.required_tags) & set(self.forbidden_tags):
            raise ValueError("required and forbidden tags must be disjoint")
        if len(set(self.allowed_confidences)) != len(self.allowed_confidences):
            raise ValueError("allowed confidences must be unique")
        return self


class TagQualitySuite(_StrictModel):
    schema_version: Literal["assistant-music-tagger-evaluation/v3"]
    id: str = Field(min_length=1, max_length=128)
    cases: list[TagQualityCase] = Field(min_length=1, max_length=100)

    @model_validator(mode="after")
    def unique_case_and_track_ids(self) -> TagQualitySuite:
        case_ids = [case.id for case in self.cases]
        if len(set(case_ids)) != len(case_ids):
            raise ValueError("case IDs must be unique within a tagging suite")
        track_ids = [case.track.track_id for case in self.cases]
        if len(set(track_ids)) != len(track_ids):
            raise ValueError("track IDs must be unique within a tagging suite")
        return self


class TagQualityCaseResult(_StrictModel):
    id: str
    description: str
    passed: bool
    tags: list[str]
    failures: list[str]


class TagQualityEvaluationResult(_StrictModel):
    schema_version: Literal["assistant-music-tagger-quality-result/v1"] = (
        "assistant-music-tagger-quality-result/v1"
    )
    suite_id: str
    engine_id: str = MODEL_TAG_ANALYZER_ID
    passed: bool
    passed_cases: int
    total_cases: int
    cases: list[TagQualityCaseResult]


class ModelTaggerError(RuntimeError):
    def __init__(self, code: str, *, diagnostic: str | None = None) -> None:
        super().__init__(code)
        self.code = code
        self.diagnostic = diagnostic


StructuredTaggerExecutor = Callable[[StructuredModelRequest], StructuredModelResult]


_TAGGING_TASK = StructuredTaskDefinition(
    task_id="assistant-music-tagger",
    role="A conservative evidence classifier for reviewable tabletop music tags.",
    objective=(
        "Choose only defensible canonical tag IDs and bounded sound axes from the "
        "supplied metadata plus independent local algorithmic evidence."
    ),
    untrusted_data=(
        "titles",
        "display titles",
        "artists",
        "albums",
        "origins",
        "genres",
    ),
    rules=numbered_rules(
        "Return every supplied track_id exactly once and no other IDs. Use no more than eight unique tag_ids for each track.",
        "Every tag_id must exactly match an ID in vocabulary. Return IDs only; never return tag names, synonyms, explanations, or invented IDs in tag_ids.",
        "Vocabulary names and definitions describe the available choices. Definitions are authoritative when labels overlap.",
        "Explicit descriptive metadata is the strongest semantic evidence. metadata_evidence is a deterministic local hypothesis, not ground truth; confirm it against the descriptive fields before using its candidate_tag_ids.",
        "metadata_evidence.tag_matches records which canonical field and explicit term produced each hypothesis. When display_title is non-empty it is the canonical title; treat conflicting raw title text cautiously.",
        "audio_evidence contains bounded signal proxies, never audio. It can support energy, brightness, tension, tempo, activity, dynamics, and rhythm, but cannot by itself prove an instrument, genre, setting, scene, culture, or D&D context.",
        "Do not turn generic high energy or tension into combat. Do not turn generic low energy into rest. Setting and scene tags require explicit semantic evidence.",
        "When evidence is sparse or conflicting, return fewer or no tags and lower confidence. Confidence describes the whole profile, not model certainty detached from evidence.",
        "All numeric axes are in the closed range 0 to 1. Evidence strings must be short factual references to supplied fields or local evidence and must not contain recommendations or hidden reasoning.",
    ),
)


def _safe_execution_error(code: str | None) -> str:
    if code is not None and _SAFE_ERROR_CODE.fullmatch(code):
        return f"model_execution_{code}"
    return "model_execution_failed"


def _metadata_evidence(
    track: ModelTagTrackInput,
    vocabulary: TagVocabularySnapshot,
) -> ModelTagMetadataEvidence:
    canonical_title = track.display_title.strip() or track.title
    canonical_title_source: Literal["display_title", "title", "none"] = (
        "display_title"
        if track.display_title.strip()
        else "title"
        if track.title.strip()
        else "none"
    )

    class _MetadataTrack:
        id = track.track_id
        path = ""
        title = canonical_title
        display_title = ""
        artist = track.artist
        album = track.album
        origin = track.origin
        genre = track.genre
        length_s = track.length_s
        bpm = track.bpm

    profile = analyze_track_metadata(_MetadataTrack())
    metadata_fields: dict[MetadataField, str] = {
        "title": canonical_title,
        "artist": track.artist,
        "album": track.album,
        "origin": track.origin,
        "genre": track.genre,
    }
    tags_by_name = vocabulary.by_name
    fields_by_tag: dict[str, set[MetadataField]] = {}
    terms_by_tag: dict[str, set[str]] = {}
    runtime_aliases = {
        tag.name: (tag.name, *tag.aliases) for tag in vocabulary.entries
    }
    for match in (
        *infer_metadata_tag_matches(metadata_fields),
        *infer_metadata_matches_for_aliases(metadata_fields, runtime_aliases),
    ):
        if match.tag not in tags_by_name:
            continue
        fields_by_tag.setdefault(match.tag, set()).update(match.matched_fields)
        terms_by_tag.setdefault(match.tag, set()).update(match.matched_terms)
    matched_tags = [
        tag for tag in vocabulary.entries if tag.name in fields_by_tag
    ]
    return ModelTagMetadataEvidence(
        analyzer_id="local-metadata-evidence/v1",
        canonical_title_source=canonical_title_source,
        candidate_tag_ids=[tag.id for tag in matched_tags],
        tag_matches=[
            ModelTagMetadataMatch(
                tag_id=tag.id,
                matched_fields=sorted(fields_by_tag[tag.name]),
                matched_terms=sorted(terms_by_tag[tag.name]),
            )
            for tag in matched_tags
        ],
        energy=profile.energy,
        brightness=profile.brightness,
        tension=profile.tension,
        confidence=profile.confidence,
    )


def _closed_tagger_schema(
    schema: dict[str, object],
    *,
    track_ids: list[int],
    tag_ids: list[str],
) -> dict[str, object]:
    closed = deepcopy(schema)
    definitions = closed.get("$defs")
    if not isinstance(definitions, dict):
        raise RuntimeError("tagger output schema is missing definitions")
    choice = definitions.get("ModelTagTrackChoice")
    if not isinstance(choice, dict):
        raise RuntimeError("tagger track choice schema is missing")
    properties = choice.get("properties")
    if not isinstance(properties, dict):
        raise RuntimeError("tagger track choice properties are missing")
    track_id_schema = properties.get("track_id")
    tag_ids_schema = properties.get("tag_ids")
    if not isinstance(track_id_schema, dict) or not isinstance(tag_ids_schema, dict):
        raise RuntimeError("tagger ID schemas are missing")
    track_id_schema["enum"] = track_ids
    tag_items = tag_ids_schema.get("items")
    if not isinstance(tag_items, dict):
        raise RuntimeError("tagger tag item schema is missing")
    tag_items["enum"] = tag_ids
    output_properties = closed.get("properties")
    if not isinstance(output_properties, dict):
        raise RuntimeError("tagger output properties are missing")
    tracks_schema = output_properties.get("tracks")
    if not isinstance(tracks_schema, dict):
        raise RuntimeError("tagger tracks schema is missing")
    tracks_schema["minItems"] = len(track_ids)
    tracks_schema["maxItems"] = len(track_ids)
    return closed


def tag_tracks(
    tracks: Sequence[ModelTagTrackInput],
    execute: StructuredTaggerExecutor,
    vocabulary: TagVocabularySnapshot | None = None,
) -> dict[int, ModelTagTrackOutput]:
    """Return one validated profile for every supplied, path-free track."""

    vocabulary = vocabulary or default_tag_vocabulary_snapshot()
    allowed_ids = vocabulary.ids
    prepared_tracks = [
        track
        if track.metadata_evidence is not None
        else track.model_copy(
            update={"metadata_evidence": _metadata_evidence(track, vocabulary)}
        )
        for track in tracks
    ]
    if any(
        track.metadata_evidence is not None
        and not set(track.metadata_evidence.candidate_tag_ids) <= allowed_ids
        for track in prepared_tracks
    ):
        raise ModelTaggerError("metadata_evidence_vocabulary_mismatch")
    groups = vocabulary.group_by_tag_id
    model_input = ModelTaggerInput(
        schema_version=MODEL_TAGGER_INPUT_CONTRACT,
        vocabulary=[
            ModelTagDefinition(
                tag_id=tag.id,
                name=tag.name,
                group=groups[tag.id].label,
                description=tag.description,
                aliases=tag.aliases,
            )
            for tag in vocabulary.entries
        ],
        tracks=prepared_tracks,
    )
    input_track_ids = [track.track_id for track in model_input.tracks]
    result = execute(
        build_structured_request(
            _TAGGING_TASK,
            model_input,
            ModelTaggerOutput,
            output_example={
                "schema_version": MODEL_TAGGER_OUTPUT_CONTRACT,
                "tracks": [
                    {
                        "track_id": track_id,
                        "tag_ids": [],
                        "energy": 0.5,
                        "brightness": 0.5,
                        "tension": 0.5,
                        "confidence": "low",
                        "evidence": [
                            "Supplied metadata is insufficient for a specific tag."
                        ],
                    }
                    for track_id in input_track_ids
                ],
            },
            max_output_tokens=_MAX_MODEL_OUTPUT_TOKENS,
            schema_transform=lambda schema: _closed_tagger_schema(
                schema,
                track_ids=input_track_ids,
                tag_ids=[tag.id for tag in vocabulary.entries],
            ),
        )
    )
    if not result.succeeded or result.payload is None:
        raise ModelTaggerError(_safe_execution_error(result.error_code))
    if result.finish_reason in {"length", "max_tokens"}:
        raise ModelTaggerError("model_output_incomplete")
    try:
        output = ModelTaggerOutput.model_validate(result.payload)
    except ValidationError as exc:
        raise ModelTaggerError(
            "model_output_schema_invalid",
            diagnostic=safe_validation_diagnostic(exc, ModelTaggerOutput),
        ) from exc
    returned_track_ids = [track.track_id for track in output.tracks]
    if returned_track_ids != input_track_ids:
        raise ModelTaggerError("model_output_track_set_mismatch")
    tags_by_id = vocabulary.by_id
    resolved: dict[int, ModelTagTrackOutput] = {}
    for track in output.tracks:
        if not set(track.tag_ids) <= allowed_ids:
            raise ModelTaggerError("model_output_unknown_tag_id")
        resolved[track.track_id] = ModelTagTrackOutput(
            track_id=track.track_id,
            tags=[tags_by_id[tag_id].name for tag_id in track.tag_ids],
            energy=track.energy,
            brightness=track.brightness,
            tension=track.tension,
            confidence=track.confidence,
            evidence=track.evidence,
        )
    return resolved


def load_tag_quality_suite(path: Path) -> TagQualitySuite:
    return TagQualitySuite.model_validate_json(path.read_text(encoding="utf-8"))


def evaluate_music_tagger(
    execute: StructuredTaggerExecutor,
    suite: TagQualitySuite,
    *,
    on_case_complete: Callable[[int, int], None] | None = None,
) -> TagQualityEvaluationResult:
    results: list[TagQualityCaseResult] = []
    total = len(suite.cases)
    for index, case in enumerate(suite.cases, start=1):
        failures: list[str] = []
        tags: list[str] = []
        try:
            profile = tag_tracks([case.track], execute)[case.track.track_id]
            tags = profile.tags
            missing = sorted(set(case.required_tags) - set(tags))
            forbidden = sorted(set(case.forbidden_tags) & set(tags))
            if missing:
                failures.append(f"Missing required tags: {', '.join(missing)}")
            if forbidden:
                failures.append(f"Returned forbidden tags: {', '.join(forbidden)}")
            if profile.confidence not in case.allowed_confidences:
                failures.append(
                    f"Returned disallowed confidence: {profile.confidence}"
                )
            if len(profile.evidence) < case.minimum_evidence_items:
                failures.append(
                    "Returned too little evidence: "
                    f"expected at least {case.minimum_evidence_items} item(s)"
                )
        except ModelTaggerError as exc:
            failure = f"Tagger error: {exc.code}"
            if exc.diagnostic is not None:
                failure += f" ({exc.diagnostic})"
            failures.append(failure)
        results.append(
            TagQualityCaseResult(
                id=case.id,
                description=case.description,
                passed=not failures,
                tags=tags,
                failures=failures,
            )
        )
        if on_case_complete is not None:
            on_case_complete(index, total)
    passed_cases = sum(result.passed for result in results)
    return TagQualityEvaluationResult(
        suite_id=suite.id,
        passed=passed_cases == total,
        passed_cases=passed_cases,
        total_cases=total,
        cases=results,
    )
