from __future__ import annotations

from dataclasses import dataclass

OPENAI_COMPATIBLE_ADAPTER = "openai-compatible/v1"


@dataclass(frozen=True)
class ProviderAdapterDefinition:
    id: str
    label: str
    description: str


@dataclass(frozen=True)
class ModelRoleDefinition:
    id: str
    label: str
    description: str


PROVIDER_ADAPTERS = (
    ProviderAdapterDefinition(
        id=OPENAI_COMPATIBLE_ADAPTER,
        label="OpenAI-compatible API",
        description=(
            "For services that expose a compatible /models endpoint and Bearer API key."
        ),
    ),
)
PROVIDER_ADAPTER_BY_ID = {adapter.id: adapter for adapter in PROVIDER_ADAPTERS}

MODEL_ROLES = (
    ModelRoleDefinition(
        id="music_tagger",
        label="Music tagging",
        description="Suggest reviewable semantic tags from approved track evidence.",
    ),
    ModelRoleDefinition(
        id="playlist_planner",
        label="Playlist planning",
        description="Interpret playlist requests and improve a reviewable local draft.",
    ),
    ModelRoleDefinition(
        id="tag_cleanup",
        label="Song-tag cleanup",
        description="Suggest consistent names and merges without changing manual tags directly.",
    ),
    ModelRoleDefinition(
        id="library_cleanup",
        label="Library cleanup",
        description="Assist the existing propose-and-review cleanup workflow.",
    ),
    ModelRoleDefinition(
        id="eq_assistant",
        label="EQ assistance",
        description="Draft strictly validated EQ presets for Authoring review.",
    ),
    ModelRoleDefinition(
        id="audio_analyzer",
        label="Specialized audio analysis",
        description="Run an optional model adapter alongside local signal analysis.",
    ),
)
MODEL_ROLE_BY_ID = {role.id: role for role in MODEL_ROLES}
