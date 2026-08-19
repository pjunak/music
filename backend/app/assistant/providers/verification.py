from __future__ import annotations

import http.client
import ipaddress
import json
import socket
import ssl
from dataclasses import dataclass
from typing import Any
from urllib.parse import urlsplit, urlunsplit

from app.assistant.providers.definitions import OPENAI_COMPATIBLE_ADAPTER

_MAX_VERIFICATION_BYTES = 1024 * 1024
_MAX_VERIFIED_MODELS = 200
_VERIFICATION_TIMEOUT_SECONDS = 10.0


class ProviderUrlError(ValueError):
    pass


@dataclass(frozen=True)
class ProviderVerificationResult:
    verified: bool
    error_code: str | None
    models: tuple[str, ...] = ()


def normalize_provider_base_url(value: str, *, allow_private_network: bool) -> str:
    raw = value.strip()
    if any(character.isspace() or ord(character) < 32 for character in raw):
        raise ProviderUrlError("Provider URL cannot contain whitespace.")
    try:
        parsed = urlsplit(raw)
        _ = parsed.port
    except ValueError as exc:
        raise ProviderUrlError("Provider URL is invalid.") from exc
    if parsed.scheme not in {"http", "https"}:
        raise ProviderUrlError("Provider URL must use HTTP or HTTPS.")
    if parsed.scheme != "https" and not allow_private_network:
        raise ProviderUrlError(
            "Public provider connections must use HTTPS."
        )
    if not parsed.hostname:
        raise ProviderUrlError("Provider URL must include a host.")
    if parsed.username is not None or parsed.password is not None:
        raise ProviderUrlError("Provider URL cannot contain credentials.")
    if parsed.query or parsed.fragment:
        raise ProviderUrlError("Provider URL cannot contain a query or fragment.")
    try:
        hostname = parsed.hostname.encode("idna").decode("ascii").lower()
    except UnicodeError as exc:
        raise ProviderUrlError("Provider URL host is invalid.") from exc
    host_for_url = f"[{hostname}]" if ":" in hostname else hostname
    netloc = f"{host_for_url}:{parsed.port}" if parsed.port is not None else host_for_url
    path = parsed.path.rstrip("/")
    return urlunsplit((parsed.scheme, netloc, path, "", ""))


def _destination_addresses(
    url: str,
    *,
    allow_private_network: bool,
) -> tuple[str, ...]:
    parsed = urlsplit(url)
    assert parsed.hostname is not None
    port = parsed.port or (443 if parsed.scheme == "https" else 80)
    try:
        addresses = {
            str(entry[4][0])
            for entry in socket.getaddrinfo(
                parsed.hostname,
                port,
                type=socket.SOCK_STREAM,
            )
        }
    except socket.gaierror:
        return ()
    if not addresses:
        return ()
    try:
        if not allow_private_network and not all(
            ipaddress.ip_address(address).is_global for address in addresses
        ):
            return ()
    except ValueError:
        return ()
    return tuple(sorted(addresses))


def _read_json_response(response: Any) -> tuple[int, object]:
    content_length = response.getheader("Content-Length")
    if content_length is not None:
        try:
            if int(content_length) > _MAX_VERIFICATION_BYTES:
                raise OverflowError
        except ValueError:
            pass
    body = response.read(_MAX_VERIFICATION_BYTES + 1)
    if len(body) > _MAX_VERIFICATION_BYTES:
        raise OverflowError
    return response.status, json.loads(body.decode("utf-8"))


def _http_get_json(
    url: str,
    api_key: str,
    addresses: tuple[str, ...],
) -> tuple[int, object]:
    parsed = urlsplit(url)
    assert parsed.hostname is not None
    port = parsed.port or (443 if parsed.scheme == "https" else 80)
    path = parsed.path or "/"
    headers = {
        "Accept": "application/json",
        "Authorization": f"Bearer {api_key}",
        "User-Agent": "music-assistant-provider-verifier/1",
    }
    last_error: OSError | None = None
    for address in addresses:
        connection: http.client.HTTPConnection | None = None
        raw_socket: socket.socket | None = None
        try:
            raw_socket = socket.create_connection(
                (address, port),
                timeout=_VERIFICATION_TIMEOUT_SECONDS,
            )
            if parsed.scheme == "https":
                context = ssl.create_default_context()
                wrapped_socket = context.wrap_socket(
                    raw_socket,
                    server_hostname=parsed.hostname,
                )
                raw_socket = None
                connection = http.client.HTTPSConnection(
                    parsed.hostname,
                    port,
                    timeout=_VERIFICATION_TIMEOUT_SECONDS,
                    context=context,
                )
                connection.sock = wrapped_socket
            else:
                connection = http.client.HTTPConnection(
                    parsed.hostname,
                    port,
                    timeout=_VERIFICATION_TIMEOUT_SECONDS,
                )
                connection.sock = raw_socket
                raw_socket = None
            connection.request("GET", path, headers=headers)
            return _read_json_response(connection.getresponse())
        except OSError as exc:
            last_error = exc
        finally:
            if connection is not None:
                connection.close()
            if raw_socket is not None:
                raw_socket.close()
    if last_error is not None:
        raise last_error
    raise OSError("provider destination has no resolved addresses")


def _safe_http_error_code(status_code: int) -> str:
    if 300 <= status_code < 400:
        return "redirect_blocked"
    if status_code == 401:
        return "unauthorized"
    if status_code == 403:
        return "forbidden"
    if status_code == 404:
        return "models_endpoint_not_found"
    if status_code == 429:
        return "rate_limited"
    return "upstream_error"


def _verify_openai_compatible(
    base_url: str,
    api_key: str,
    *,
    allow_private_network: bool,
) -> ProviderVerificationResult:
    models_url = f"{base_url.rstrip('/')}/models"
    addresses = _destination_addresses(
        models_url,
        allow_private_network=allow_private_network,
    )
    if not addresses:
        return ProviderVerificationResult(False, "destination_blocked")
    try:
        status_code, payload = _http_get_json(models_url, api_key, addresses)
    except OverflowError:
        return ProviderVerificationResult(False, "response_too_large")
    except (json.JSONDecodeError, UnicodeDecodeError, TypeError):
        return ProviderVerificationResult(False, "invalid_response")
    except TimeoutError:
        return ProviderVerificationResult(False, "timeout")
    except ssl.SSLError:
        return ProviderVerificationResult(False, "tls_error")
    except (OSError, http.client.HTTPException):
        return ProviderVerificationResult(False, "network_error")

    if not 200 <= status_code < 300:
        return ProviderVerificationResult(False, _safe_http_error_code(status_code))
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
    return ProviderVerificationResult(True, None, tuple(models))


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
