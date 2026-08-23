from __future__ import annotations

from dataclasses import dataclass

OPENAI_COMPATIBLE_ADAPTER = "openai-compatible/v1"
OPENAI_COMPATIBLE_JSON_SCHEMA_ADAPTER = "openai-compatible-json-schema/v1"
STRUCTURED_TEXT_CAPABILITY = "structured-text/v1"
STRICT_JSON_SCHEMA_CAPABILITY = "strict-json-schema/v1"
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
        id=STRICT_JSON_SCHEMA_CAPABILITY,
        label="Strict JSON Schema",
        description=(
            "Constrains model responses with the task's exact JSON Schema at the API."
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
            "Maximum compatibility using JSON-object response mode plus strict local validation."
        ),
        capability_ids=(STRUCTURED_TEXT_CAPABILITY,),
    ),
    ProviderAdapterDefinition(
        id=OPENAI_COMPATIBLE_JSON_SCHEMA_ADAPTER,
        label="OpenAI-compatible strict JSON Schema",
        description=(
            "For compatible services that support response_format type json_schema. "
            "Use the standard adapter when the provider supports only json_object."
        ),
        capability_ids=(STRUCTURED_TEXT_CAPABILITY, STRICT_JSON_SCHEMA_CAPABILITY),
    ),
)
PROVIDER_ADAPTER_BY_ID = {adapter.id: adapter for adapter in PROVIDER_ADAPTERS}

MODEL_ROLES = (
    ModelRoleDefinition(
        id="music_tagger",
        label="Mood tagging",
        description=(
            "Suggest reviewable terrain, scene, and mood database tags from approved track evidence."
        ),
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
        label="Mood-tag cleanup",
        description=(
            "Suggests review-only consistent names and merges from the mood-tag catalog."
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
        description="Creates bounded graphic-EQ drafts for explicit Authoring review.",
        required_capability_ids=(STRUCTURED_TEXT_CAPABILITY,),
        configuration_available=True,
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

# Feature prompt/input/output changes must invalidate conformance and quality
# results even when a connection, model, and runtime limits are unchanged.
MODEL_ROLE_RUNTIME_CONTRACTS: dict[str, str] = {
    "music_tagger": (
        "assistant-music-tagger-input/v8+output/v2+evidence-canonicalization/v1"
    ),
    "playlist_planner": "assistant-playlist-planner-input/v2+output/v1+closed-ids/v1",
    "tag_cleanup": "assistant-model-tag-cleanup-input/v3+output/v2",
    "library_cleanup": "reserved-library-cleanup/v1",
    "eq_assistant": "assistant-eq-draft-input/v2+output/v1",
    "audio_analyzer": "reserved-audio-analyzer/v1",
}
