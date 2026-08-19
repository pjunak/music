from __future__ import annotations

from dataclasses import dataclass

from app.assistant.providers.definitions import (
    OPENAI_COMPATIBLE_ADAPTER,
    STRUCTURED_TEXT_CAPABILITY,
)
from app.assistant.providers.transport import (
    ProviderTransportError,
    ProviderUrlError,
    normalize_provider_base_url,
    request_json,
    safe_http_error_code,
)

_MAX_VERIFICATION_BYTES = 1024 * 1024
_MAX_VERIFIED_MODELS = 200
_VERIFICATION_TIMEOUT_SECONDS = 10.0

__all__ = [
    "ProviderUrlError",
    "ProviderVerificationResult",
    "normalize_provider_base_url",
    "verify_provider_connection",
]


@dataclass(frozen=True)
class ProviderVerificationResult:
    verified: bool
    error_code: str | None
    models: tuple[str, ...] = ()
    capability_ids: tuple[str, ...] = ()


def _verify_openai_compatible(
    base_url: str,
    api_key: str,
    *,
    allow_private_network: bool,
) -> ProviderVerificationResult:
    try:
        response = request_json(
            "GET",
            f"{base_url.rstrip('/')}/models",
            api_key,
            allow_private_network=allow_private_network,
            timeout_seconds=_VERIFICATION_TIMEOUT_SECONDS,
            max_response_bytes=_MAX_VERIFICATION_BYTES,
            user_agent="music-assistant-provider-verifier/1",
        )
    except ProviderTransportError as exc:
        return ProviderVerificationResult(False, exc.code)

    if not 200 <= response.status_code < 300:
        return ProviderVerificationResult(
            False,
            safe_http_error_code(
                response.status_code,
                not_found_code="models_endpoint_not_found",
            ),
        )
    payload = response.payload
    if not isinstance(payload, dict) or not isinstance(payload.get("data"), list):
        return ProviderVerificationResult(False, "invalid_response")
    models: list[str] = []
    for item in payload["data"]:
        if not isinstance(item, dict):
            continue
        model_id = item.get("id")
        if (
            isinstance(model_id, str)
            and 0 < len(model_id) <= 256
            and model_id not in models
        ):
            models.append(model_id)
        if len(models) >= _MAX_VERIFIED_MODELS:
            break
    return ProviderVerificationResult(
        True,
        None,
        tuple(models),
        (STRUCTURED_TEXT_CAPABILITY,),
    )


def verify_provider_connection(
    adapter_id: str,
    base_url: str,
    api_key: str,
    *,
    allow_private_network: bool,
) -> ProviderVerificationResult:
    if adapter_id == OPENAI_COMPATIBLE_ADAPTER:
        return _verify_openai_compatible(
            base_url,
            api_key,
            allow_private_network=allow_private_network,
        )
    return ProviderVerificationResult(False, "unsupported_adapter")
