from __future__ import annotations

from dataclasses import dataclass

OPENAI_COMPATIBLE_ADAPTER = "openai-compatible/v1"
STRUCTURED_TEXT_CAPABILITY = "structured-text/v1"
AUDIO_INPUT_CAPABILITY = "audio-input/v1"


@dataclass(frozen=True)
class ProviderCapabilityDefinition:
    id: str
    label: str
    description: str


@dataclass(frozen=True)
class ProviderAdapterDefinition:
    id: str
    label: str
    description: str
    capability_ids: tuple[str, ...]


@dataclass(frozen=True)
class ModelRoleDefinition:
    id: str
    label: str
    description: str
    required_capability_ids: tuple[str, ...]
    configuration_available: bool


PROVIDER_CAPABILITIES = (
    ProviderCapabilityDefinition(
        id=STRUCTURED_TEXT_CAPABILITY,
        label="Structured text",
        description=(
            "Sends text instructions and receives a validated machine-readable result."
        ),
    ),
    ProviderCapabilityDefinition(
        id=AUDIO_INPUT_CAPABILITY,
        label="Audio input",
        description="Accepts bounded audio content through a dedicated provider adapter.",
    ),
)
PROVIDER_CAPABILITY_BY_ID = {
    capability.id: capability for capability in PROVIDER_CAPABILITIES
}


PROVIDER_ADAPTERS = (
    ProviderAdapterDefinition(
        id=OPENAI_COMPATIBLE_ADAPTER,
        label="OpenAI-compatible API",
        description=(
            "For services that expose a compatible /models endpoint and Bearer API key."
        ),
        capability_ids=(STRUCTURED_TEXT_CAPABILITY,),
    ),
)
PROVIDER_ADAPTER_BY_ID = {adapter.id: adapter for adapter in PROVIDER_ADAPTERS}

MODEL_ROLES = (
    ModelRoleDefinition(
        id="music_tagger",
        label="Music tagging",
        description="Suggest reviewable semantic tags from approved track evidence.",
        required_capability_ids=(STRUCTURED_TEXT_CAPABILITY,),
        configuration_available=True,
    ),
    ModelRoleDefinition(
        id="playlist_planner",
        label="Playlist planning",
        description="Interpret playlist requests and improve a reviewable local draft.",
        required_capability_ids=(STRUCTURED_TEXT_CAPABILITY,),
        configuration_available=True,
    ),
    ModelRoleDefinition(
        id="tag_cleanup",
        label="Song-tag cleanup",
        description=(
            "Suggests review-only consistent names and merges from the manual tag catalog."
        ),
        required_capability_ids=(STRUCTURED_TEXT_CAPABILITY,),
        configuration_available=True,
    ),
    ModelRoleDefinition(
        id="library_cleanup",
        label="Library cleanup",
        description=(
            "Reserved for a future model pass over the existing review-first cleanup."
        ),
        required_capability_ids=(STRUCTURED_TEXT_CAPABILITY,),
        configuration_available=False,
    ),
    ModelRoleDefinition(
        id="eq_assistant",
        label="EQ assistance",
        description="Reserved for future validated EQ drafts in Authoring.",
        required_capability_ids=(STRUCTURED_TEXT_CAPABILITY,),
        configuration_available=False,
    ),
    ModelRoleDefinition(
        id="audio_analyzer",
        label="Specialized audio analysis",
        description=(
            "Reserved for a future audio-capable adapter with separate consent."
        ),
        required_capability_ids=(AUDIO_INPUT_CAPABILITY,),
        configuration_available=False,
    ),
)
MODEL_ROLE_BY_ID = {role.id: role for role in MODEL_ROLES}
