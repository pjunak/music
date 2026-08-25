"""Provider-specific request shaping behind Music's hardened HTTP transport.

Handlers deliberately do not perform network I/O. Verification and execution
continue to share the pinned-DNS, no-redirect, bounded transport in
``providers.transport`` while this registry owns documented wire quirks such as
model resource names and thinking parameters.
"""

from copy import deepcopy
from dataclasses import dataclass
from typing import Literal

from app.assistant.providers.definitions import (
    GOOGLE_GEMINI_OPENAI_ADAPTER,
    GOOGLE_GEMINI_OPENAI_JSON_SCHEMA_ADAPTER,
    OPENAI_COMPATIBLE_ADAPTER,
    OPENAI_COMPATIBLE_JSON_SCHEMA_ADAPTER,
    OPENAI_RESPONSES_ADAPTER,
)

StructuredOutputMode = Literal["json_object", "json_schema"]
ThinkingParameterStyle = Literal[
    "thinking_object",
    "reasoning_effort",
    "reasoning_object",
]
ExecutionApiStyle = Literal["chat_completions", "responses"]
ThinkingMode = Literal["provider_default", "enabled", "disabled"]
OutputSchemaDialect = Literal["full", "gemini_subset"]

GOOGLE_GEMINI_OPENAI_BASE_URL = "https://generativelanguage.googleapis.com/v1beta/openai"
OPENAI_API_BASE_URL = "https://api.openai.com/v1"


def _gemini_compatible_output_schema(value: object) -> object:
    """Project canonical JSON Schema into Gemini's documented subset.

    Feature code and the shared prompt continue to own the complete generated
    schema. Gemini structured output accepts only a subset of JSON Schema, so
    its wire schema omits unsupported validation annotations. The unchanged
    task model still validates every returned payload locally.
    """

    if isinstance(value, list):
        return [_gemini_compatible_output_schema(item) for item in value]
    if not isinstance(value, dict):
        return deepcopy(value)

    compatible: dict[str, object] = {}
    for key, item in value.items():
        if key in {
            "exclusiveMinimum",
            "exclusiveMaximum",
            "maxLength",
            "minLength",
            "multipleOf",
            "pattern",
            "uniqueItems",
        }:
            continue
        if key == "const":
            # Gemini documents enum for string and numeric values, but not
            # const. Boolean const constraints remain enforced by the prompt
            # and local task validation rather than adding an undocumented
            # boolean enum to the provider request.
            if isinstance(item, (str, int, float)) and not isinstance(item, bool):
                compatible["enum"] = [deepcopy(item)]
            continue
        compatible[key] = _gemini_compatible_output_schema(item)
    return compatible


class ProviderHandlerConfigurationError(ValueError):
    """Raised when connection settings contradict an explicit handler profile."""


@dataclass(frozen=True)
class ProviderAdapterHandler:
    adapter_id: str
    structured_output_mode: StructuredOutputMode
    thinking_parameter_style: ThinkingParameterStyle
    execution_api_style: ExecutionApiStyle = "chat_completions"
    expected_base_url: str | None = None
    model_resource_prefix: str | None = None
    models_path: str = "/models"
    completion_path: str = "/chat/completions"
    additional_headers: tuple[tuple[str, str], ...] = ()
    output_schema_dialect: OutputSchemaDialect = "full"

    def validate_base_url(self, base_url: str) -> None:
        if self.expected_base_url is not None and base_url != self.expected_base_url:
            raise ProviderHandlerConfigurationError(
                f"This connection type requires {self.expected_base_url}."
            )

    def models_url(self, base_url: str) -> str:
        return f"{base_url.rstrip('/')}{self.models_path}"

    def completion_url(self, base_url: str) -> str:
        return f"{base_url.rstrip('/')}{self.completion_path}"

    def normalize_model_id(self, model_id: str) -> str:
        prefix = self.model_resource_prefix
        if prefix is not None and model_id.startswith(prefix):
            normalized = model_id.removeprefix(prefix)
            if normalized:
                return normalized
        return model_id

    def prepare_output_schema(
        self,
        output_schema: dict[str, object],
    ) -> dict[str, object]:
        if self.output_schema_dialect == "full":
            return output_schema
        compatible = _gemini_compatible_output_schema(output_schema)
        assert isinstance(compatible, dict)
        return compatible

    def apply_thinking_mode(
        self,
        payload: dict[str, object],
        thinking_mode: ThinkingMode,
    ) -> None:
        if thinking_mode == "provider_default":
            return
        if self.thinking_parameter_style == "reasoning_effort":
            payload["reasoning_effort"] = "high" if thinking_mode == "enabled" else "none"
            return
        if self.thinking_parameter_style == "reasoning_object":
            payload["reasoning"] = {
                "effort": "high" if thinking_mode == "enabled" else "none"
            }
            return
        # Preserve the existing extension used by already-configured compatible
        # services. New provider-specific variations belong in an explicit
        # versioned handler instead of URL or model-name inference here.
        payload["thinking"] = {"type": thinking_mode}


PROVIDER_ADAPTER_HANDLERS = (
    ProviderAdapterHandler(
        adapter_id=OPENAI_RESPONSES_ADAPTER,
        structured_output_mode="json_schema",
        thinking_parameter_style="reasoning_object",
        execution_api_style="responses",
        expected_base_url=OPENAI_API_BASE_URL,
        completion_path="/responses",
    ),
    ProviderAdapterHandler(
        adapter_id=OPENAI_COMPATIBLE_ADAPTER,
        structured_output_mode="json_object",
        thinking_parameter_style="thinking_object",
    ),
    ProviderAdapterHandler(
        adapter_id=OPENAI_COMPATIBLE_JSON_SCHEMA_ADAPTER,
        structured_output_mode="json_schema",
        thinking_parameter_style="thinking_object",
    ),
    ProviderAdapterHandler(
        adapter_id=GOOGLE_GEMINI_OPENAI_ADAPTER,
        structured_output_mode="json_schema",
        thinking_parameter_style="reasoning_effort",
        expected_base_url=GOOGLE_GEMINI_OPENAI_BASE_URL,
        model_resource_prefix="models/",
        additional_headers=(("x-goog-api-client", "music-assistant-oai/1.0"),),
        output_schema_dialect="gemini_subset",
    ),
    ProviderAdapterHandler(
        adapter_id=GOOGLE_GEMINI_OPENAI_JSON_SCHEMA_ADAPTER,
        structured_output_mode="json_schema",
        thinking_parameter_style="reasoning_effort",
        expected_base_url=GOOGLE_GEMINI_OPENAI_BASE_URL,
        model_resource_prefix="models/",
        additional_headers=(("x-goog-api-client", "music-assistant-oai/1.0"),),
        output_schema_dialect="gemini_subset",
    ),
)
PROVIDER_ADAPTER_HANDLER_BY_ID = {
    handler.adapter_id: handler for handler in PROVIDER_ADAPTER_HANDLERS
}


def get_provider_adapter_handler(adapter_id: str) -> ProviderAdapterHandler | None:
    return PROVIDER_ADAPTER_HANDLER_BY_ID.get(adapter_id)


def validate_provider_handler_base_url(adapter_id: str, base_url: str) -> None:
    handler = get_provider_adapter_handler(adapter_id)
    if handler is None:
        raise ProviderHandlerConfigurationError(
            "This connection type does not have a provider handler."
        )
    handler.validate_base_url(base_url)
