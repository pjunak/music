"""Bounded, DNS-pinned HTTP transport for optional model providers.

This module is deliberately unaware of provider response shapes and Assistant
features. Verification and model adapters share it so every outbound request
gets the same redirect, destination, timeout, and size protections.
"""

import http.client
import ipaddress
import json
import socket
import ssl
from collections.abc import Mapping
from dataclasses import dataclass
from typing import Any, Literal
from urllib.parse import urlsplit, urlunsplit

_MAX_REQUEST_BYTES = 256 * 1024
_ALLOWED_ADDITIONAL_HEADERS = {"x-goog-api-client"}
_SAFE_PROVIDER_ERROR_CODES = {
    "api_error": "upstream_error",
    "authentication_error": "unauthorized",
    "deadline_exceeded": "provider_timeout",
    "failed_precondition": "failed_precondition",
    "insufficient_quota": "quota_exceeded",
    "invalid_api_key": "unauthorized",
    "invalid_argument": "invalid_request",
    "invalid_request": "invalid_request",
    "invalid_request_error": "invalid_request",
    "invalid_value": "invalid_request",
    "model_not_found": "model_not_found",
    "parameter_unknown": "parameter_unknown",
    "permission_denied": "forbidden",
    "quota_exceeded": "quota_exceeded",
    "rate_limit_exceeded": "rate_limited",
    "resource_exhausted": "rate_limited",
    "server_error": "upstream_error",
    "service_unavailable": "service_unavailable",
    "unavailable": "service_unavailable",
    "unimplemented": "unsupported_provider_feature",
    "unsupported_parameter": "parameter_unknown",
}


class ProviderUrlError(ValueError):
    pass


class ProviderTransportError(RuntimeError):
    def __init__(self, code: str) -> None:
        super().__init__(code)
        self.code = code


@dataclass(frozen=True)
class JsonHttpResponse:
    status_code: int
    payload: object


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
        raise ProviderUrlError("Public provider connections must use HTTPS.")
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


def _read_json_response(
    response: Any,
    *,
    max_response_bytes: int,
) -> object:
    content_length = response.getheader("Content-Length")
    if content_length is not None:
        try:
            if int(content_length) > max_response_bytes:
                raise ProviderTransportError("response_too_large")
        except ValueError:
            pass
    body = response.read(max_response_bytes + 1)
    if len(body) > max_response_bytes:
        raise ProviderTransportError("response_too_large")
    try:
        return json.loads(body.decode("utf-8"))
    except (json.JSONDecodeError, UnicodeDecodeError) as exc:
        raise ProviderTransportError("invalid_response") from exc


def _http_json(
    method: Literal["GET", "POST"],
    url: str,
    api_key: str,
    addresses: tuple[str, ...],
    *,
    body: bytes | None,
    timeout_seconds: float,
    max_response_bytes: int,
    user_agent: str,
    additional_headers: Mapping[str, str] | None = None,
) -> JsonHttpResponse:
    parsed = urlsplit(url)
    assert parsed.hostname is not None
    port = parsed.port or (443 if parsed.scheme == "https" else 80)
    path = parsed.path or "/"
    headers = {
        "Accept": "application/json",
        "Authorization": f"Bearer {api_key}",
        "User-Agent": user_agent,
    }
    for name, value in (additional_headers or {}).items():
        normalized_name = name.lower()
        if (
            normalized_name not in _ALLOWED_ADDITIONAL_HEADERS
            or not value
            or len(value) > 256
            or any(character in "\r\n" or ord(character) < 32 for character in value)
        ):
            raise ProviderTransportError("invalid_request_headers")
        headers[name] = value
    if body is not None:
        headers["Content-Type"] = "application/json"
        headers["Content-Length"] = str(len(body))

    last_error: OSError | http.client.HTTPException | None = None
    for address in addresses:
        connection: http.client.HTTPConnection | None = None
        raw_socket: socket.socket | None = None
        try:
            raw_socket = socket.create_connection(
                (address, port),
                timeout=timeout_seconds,
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
                    timeout=timeout_seconds,
                    context=context,
                )
                connection.sock = wrapped_socket
            else:
                connection = http.client.HTTPConnection(
                    parsed.hostname,
                    port,
                    timeout=timeout_seconds,
                )
                connection.sock = raw_socket
                raw_socket = None
            connection.request(method, path, body=body, headers=headers)
            response = connection.getresponse()
            try:
                payload = _read_json_response(
                    response,
                    max_response_bytes=max_response_bytes,
                )
            except ProviderTransportError as exc:
                if 200 <= response.status < 300 or exc.code != "invalid_response":
                    raise
                payload = None
            return JsonHttpResponse(response.status, payload)
        except ssl.SSLError as exc:
            raise ProviderTransportError("tls_error") from exc
        except TimeoutError as exc:
            raise ProviderTransportError("timeout") from exc
        except ProviderTransportError:
            raise
        except (OSError, http.client.HTTPException) as exc:
            last_error = exc
        finally:
            if connection is not None:
                connection.close()
            if raw_socket is not None:
                raw_socket.close()
    if last_error is not None:
        raise ProviderTransportError("network_error") from last_error
    raise ProviderTransportError("network_error")


def request_json(
    method: Literal["GET", "POST"],
    url: str,
    api_key: str,
    *,
    allow_private_network: bool,
    timeout_seconds: float,
    max_response_bytes: int,
    user_agent: str,
    payload: object | None = None,
    additional_headers: Mapping[str, str] | None = None,
) -> JsonHttpResponse:
    """Send one JSON request without redirects or a second DNS lookup."""

    addresses = _destination_addresses(
        url,
        allow_private_network=allow_private_network,
    )
    if not addresses:
        raise ProviderTransportError("destination_blocked")
    body: bytes | None = None
    if payload is not None:
        body = json.dumps(
            payload,
            ensure_ascii=False,
            separators=(",", ":"),
        ).encode("utf-8")
        if len(body) > _MAX_REQUEST_BYTES:
            raise ProviderTransportError("request_too_large")
    return _http_json(
        method,
        url,
        api_key,
        addresses,
        body=body,
        timeout_seconds=timeout_seconds,
        max_response_bytes=max_response_bytes,
        user_agent=user_agent,
        additional_headers=additional_headers,
    )


def safe_provider_error_code(payload: object, *, fallback: str) -> str:
    """Return only an allowlisted provider error classification, never its message."""

    if not isinstance(payload, dict):
        return fallback
    error = payload.get("error")
    details = error if isinstance(error, dict) else payload
    for key in ("code", "type", "status"):
        value = details.get(key)
        if not isinstance(value, str) or len(value) > 128:
            continue
        normalized = value.strip().lower()
        mapped = _SAFE_PROVIDER_ERROR_CODES.get(normalized)
        if mapped is not None:
            return mapped
    return fallback


def safe_http_error_code(
    status_code: int,
    *,
    not_found_code: str,
    payload: object = None,
) -> str:
    if 300 <= status_code < 400:
        return "redirect_blocked"
    if status_code == 401:
        return "unauthorized"
    if status_code == 403:
        return "forbidden"
    provider_code = safe_provider_error_code(payload, fallback="")
    if provider_code:
        return provider_code
    if status_code == 404:
        return not_found_code
    if status_code == 429:
        return "rate_limited"
    if status_code in {400, 422}:
        return "invalid_request"
    return "upstream_error"
