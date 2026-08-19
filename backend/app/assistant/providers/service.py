from __future__ import annotations

import hashlib
import json
import secrets
from dataclasses import dataclass
from typing import Literal, cast

from sqlalchemy import func, select
from sqlalchemy.exc import IntegrityError
from sqlalchemy.orm import Session

from app.assistant.providers.credentials import (
    CredentialVault,
    CredentialVaultError,
    credential_vault_status,
)
from app.assistant.providers.definitions import (
    MODEL_ROLE_BY_ID,
    MODEL_ROLES,
    PROVIDER_ADAPTER_BY_ID,
    PROVIDER_ADAPTERS,
)
from app.assistant.providers.schemas import (
    ModelRoleDefinitionOut,
    ModelRoleOut,
    ModelRoleUpdate,
    ProviderAdapterOut,
    ProviderConnectionCreate,
    ProviderConnectionOut,
    ProviderConnectionUpdate,
    ProviderFrameworkStatusOut,
)
from app.assistant.providers.verification import (
    ProviderUrlError,
    ProviderVerificationResult,
    normalize_provider_base_url,
)
from app.models.assistant_model_role import AssistantModelRole
from app.models.assistant_provider_connection import AssistantProviderConnection
from app.models.base import utcnow


class ProviderServiceError(RuntimeError):
    def __init__(self, code: str, message: str, status_code: int) -> None:
        super().__init__(message)
        self.code = code
        self.message = message
        self.status_code = status_code


@dataclass(frozen=True)
class VerificationTarget:
    connection_id: str
    adapter_id: str
    base_url: str
    api_key: str
    allow_private_network: bool
    fingerprint: str


def _service_error_from_vault(error: CredentialVaultError) -> ProviderServiceError:
    if error.code == "master_key_not_configured":
        return ProviderServiceError(
            error.code,
            "Provider credential storage is not configured on this server.",
            503,
        )
    if error.code == "invalid_master_key":
        return ProviderServiceError(
            error.code,
            "The server provider-credential master key is invalid.",
            503,
        )
    return ProviderServiceError(
        error.code,
        "The stored provider credential cannot be decrypted.",
        503,
    )


def _vault() -> CredentialVault:
    try:
        return CredentialVault.from_settings()
    except CredentialVaultError as exc:
        raise _service_error_from_vault(exc) from None


def _adapter_exists(adapter_id: str) -> None:
    if adapter_id not in PROVIDER_ADAPTER_BY_ID:
        raise ProviderServiceError(
            "unsupported_adapter",
            "That provider adapter is not supported.",
            422,
        )


def _unique_name(db: Session, name: str, *, excluding_id: str | None = None) -> None:
    statement = select(AssistantProviderConnection.id).where(
        func.lower(AssistantProviderConnection.name) == name.casefold()
    )
    if excluding_id is not None:
        statement = statement.where(AssistantProviderConnection.id != excluding_id)
    if db.scalar(statement) is not None:
        raise ProviderServiceError(
            "duplicate_connection_name",
            "A provider connection with that name already exists.",
            409,
        )


def _models(row: AssistantProviderConnection) -> list[str]:
    try:
        value = json.loads(row.verified_models_json)
    except json.JSONDecodeError:
        return []
    if not isinstance(value, list):
        return []
    return [item for item in value if isinstance(item, str) and 0 < len(item) <= 256]


def _verification_status(
    value: str,
) -> Literal["never", "verified", "failed"]:
    return cast(
        "Literal['never', 'verified', 'failed']",
        value if value in {"never", "verified", "failed"} else "failed",
    )


def connection_out(row: AssistantProviderConnection) -> ProviderConnectionOut:
    return ProviderConnectionOut(
        id=row.id,
        name=row.name,
        adapter_id=row.adapter_id,
        base_url=row.base_url,
        key_hint=row.api_key_hint,
        allow_private_network=row.allow_private_network,
        verification_status=_verification_status(row.verification_status),
        verification_error_code=row.verification_error_code,
        verified_models=_models(row),
        last_verified_at=row.last_verified_at,
        created_at=row.created_at,
        updated_at=row.updated_at,
    )


def framework_status() -> ProviderFrameworkStatusOut:
    ready, error = credential_vault_status()
    return ProviderFrameworkStatusOut(
        credential_storage_ready=ready,
        credential_storage_error=error,
        adapters=[
            ProviderAdapterOut(
                id=adapter.id,
                label=adapter.label,
                description=adapter.description,
            )
            for adapter in PROVIDER_ADAPTERS
        ],
        roles=[
            ModelRoleDefinitionOut(
                id=role.id,
                label=role.label,
                description=role.description,
            )
            for role in MODEL_ROLES
        ],
    )


def list_connections(db: Session) -> list[ProviderConnectionOut]:
    rows = db.scalars(
        select(AssistantProviderConnection).order_by(
            func.lower(AssistantProviderConnection.name),
            AssistantProviderConnection.id,
        )
    ).all()
    return [connection_out(row) for row in rows]


def get_connection(db: Session, connection_id: str) -> AssistantProviderConnection:
    row = db.get(AssistantProviderConnection, connection_id)
    if row is None:
        raise ProviderServiceError(
            "connection_not_found",
            "Provider connection not found.",
            404,
        )
    return row


def create_connection(
    db: Session,
    payload: ProviderConnectionCreate,
) -> ProviderConnectionOut:
    _adapter_exists(payload.adapter_id)
    _unique_name(db, payload.name)
    try:
        base_url = normalize_provider_base_url(
            payload.base_url,
            allow_private_network=payload.allow_private_network,
        )
    except ProviderUrlError as exc:
        raise ProviderServiceError("invalid_provider_url", str(exc), 422) from None
    connection_id = secrets.token_hex(16)
    try:
        encrypted = _vault().encrypt(
            connection_id,
            payload.api_key.get_secret_value(),
        )
    except CredentialVaultError as exc:
        raise _service_error_from_vault(exc) from None
    row = AssistantProviderConnection(
        id=connection_id,
        name=payload.name,
        adapter_id=payload.adapter_id,
        base_url=base_url,
        encrypted_api_key=encrypted.ciphertext,
        api_key_nonce=encrypted.nonce,
        api_key_hint=encrypted.hint,
        allow_private_network=payload.allow_private_network,
    )
    db.add(row)
    try:
        db.commit()
    except IntegrityError:
        db.rollback()
        raise ProviderServiceError(
            "duplicate_connection_name",
            "A provider connection with that name already exists.",
            409,
        ) from None
    db.refresh(row)
    return connection_out(row)


def update_connection(
    db: Session,
    connection_id: str,
    payload: ProviderConnectionUpdate,
) -> ProviderConnectionOut:
    row = get_connection(db, connection_id)
    name = payload.name if payload.name is not None else row.name
    adapter_id = payload.adapter_id if payload.adapter_id is not None else row.adapter_id
    base_url_value = payload.base_url if payload.base_url is not None else row.base_url
    allow_private = (
        payload.allow_private_network
        if payload.allow_private_network is not None
        else row.allow_private_network
    )
    _adapter_exists(adapter_id)
    _unique_name(db, name, excluding_id=row.id)
    try:
        base_url = normalize_provider_base_url(
            base_url_value,
            allow_private_network=allow_private,
        )
    except ProviderUrlError as exc:
        raise ProviderServiceError("invalid_provider_url", str(exc), 422) from None

    verification_inputs_changed = (
        adapter_id != row.adapter_id
        or base_url != row.base_url
        or allow_private != row.allow_private_network
        or payload.api_key is not None
    )
    row.name = name
    row.adapter_id = adapter_id
    row.base_url = base_url
    row.allow_private_network = allow_private
    if payload.api_key is not None:
        try:
            encrypted = _vault().encrypt(
                row.id,
                payload.api_key.get_secret_value(),
            )
        except CredentialVaultError as exc:
            raise _service_error_from_vault(exc) from None
        row.encrypted_api_key = encrypted.ciphertext
        row.api_key_nonce = encrypted.nonce
        row.api_key_hint = encrypted.hint
    if verification_inputs_changed:
        row.verification_status = "never"
        row.verification_error_code = None
        row.verified_models_json = "[]"
        row.last_verified_at = None
    row.updated_at = utcnow()
    try:
        db.commit()
    except IntegrityError:
        db.rollback()
        raise ProviderServiceError(
            "duplicate_connection_name",
            "A provider connection with that name already exists.",
            409,
        ) from None
    db.refresh(row)
    return connection_out(row)


def delete_connection(db: Session, connection_id: str) -> None:
    row = get_connection(db, connection_id)
    assigned = db.scalar(
        select(AssistantModelRole.role_id).where(
            AssistantModelRole.connection_id == connection_id
        )
    )
    if assigned is not None:
        raise ProviderServiceError(
            "connection_in_use",
            "Remove this connection from its model roles before deleting it.",
            409,
        )
    db.delete(row)
    db.commit()


def _connection_fingerprint(row: AssistantProviderConnection) -> str:
    payload = "\0".join(
        (
            row.id,
            row.adapter_id,
            row.base_url,
            row.encrypted_api_key,
            row.api_key_nonce,
            "1" if row.allow_private_network else "0",
        )
    )
    return hashlib.sha256(payload.encode("utf-8")).hexdigest()


def prepare_verification(db: Session, connection_id: str) -> VerificationTarget:
    row = get_connection(db, connection_id)
    try:
        api_key = _vault().decrypt(
            row.id,
            row.encrypted_api_key,
            row.api_key_nonce,
        )
    except CredentialVaultError as exc:
        raise _service_error_from_vault(exc) from None
    return VerificationTarget(
        connection_id=row.id,
        adapter_id=row.adapter_id,
        base_url=row.base_url,
        api_key=api_key,
        allow_private_network=row.allow_private_network,
        fingerprint=_connection_fingerprint(row),
    )


def finish_verification(
    db: Session,
    target: VerificationTarget,
    result: ProviderVerificationResult,
) -> ProviderConnectionOut:
    db.expire_all()
    row = get_connection(db, target.connection_id)
    if _connection_fingerprint(row) != target.fingerprint:
        raise ProviderServiceError(
            "connection_changed",
            "The connection changed while verification was running. Verify it again.",
            409,
        )
    row.verification_status = "verified" if result.verified else "failed"
    row.verification_error_code = result.error_code
    row.verified_models_json = json.dumps(
        list(result.models) if result.verified else [],
        ensure_ascii=False,
        separators=(",", ":"),
    )
    row.last_verified_at = utcnow()
    row.updated_at = row.last_verified_at
    db.commit()
    db.refresh(row)
    return connection_out(row)


def _role_out(
    definition_id: str,
    row: AssistantModelRole | None,
    connection: AssistantProviderConnection | None,
    *,
    credential_available: bool,
) -> ModelRoleOut:
    definition = MODEL_ROLE_BY_ID[definition_id]
    verification_status = (
        _verification_status(connection.verification_status)
        if connection is not None
        else None
    )
    return ModelRoleOut(
        role_id=definition.id,
        label=definition.label,
        description=definition.description,
        connection_id=row.connection_id if row is not None else None,
        connection_name=connection.name if connection is not None else None,
        model_id=row.model_id if row is not None else "",
        enabled=row.enabled if row is not None else False,
        effective_enabled=(
            row is not None
            and row.enabled
            and verification_status == "verified"
            and credential_available
        ),
        timeout_seconds=row.timeout_seconds if row is not None else 30,
        max_output_tokens=row.max_output_tokens if row is not None else 2_000,
        verification_status=verification_status,
        updated_at=row.updated_at if row is not None else None,
    )


def list_model_roles(db: Session) -> list[ModelRoleOut]:
    rows = {
        row.role_id: row
        for row in db.scalars(select(AssistantModelRole)).all()
        if row.role_id in MODEL_ROLE_BY_ID
    }
    connection_ids = {row.connection_id for row in rows.values()}
    connections = (
        {
            row.id: row
            for row in db.scalars(
                select(AssistantProviderConnection).where(
                    AssistantProviderConnection.id.in_(connection_ids)
                )
            ).all()
        }
        if connection_ids
        else {}
    )
    readable_connection_ids: set[str] = set()
    try:
        vault = _vault()
    except ProviderServiceError:
        vault = None
    if vault is not None:
        for connection in connections.values():
            try:
                vault.decrypt(
                    connection.id,
                    connection.encrypted_api_key,
                    connection.api_key_nonce,
                )
            except CredentialVaultError:
                continue
            readable_connection_ids.add(connection.id)
    return [
        _role_out(
            definition.id,
            rows.get(definition.id),
            connections.get(rows[definition.id].connection_id)
            if definition.id in rows
            else None,
            credential_available=(
                definition.id in rows
                and rows[definition.id].connection_id in readable_connection_ids
            ),
        )
        for definition in MODEL_ROLES
    ]


def update_model_role(
    db: Session,
    role_id: str,
    payload: ModelRoleUpdate,
) -> ModelRoleOut:
    if role_id not in MODEL_ROLE_BY_ID:
        raise ProviderServiceError("role_not_found", "Model role not found.", 404)
    connection = get_connection(db, payload.connection_id)
    if payload.enabled and connection.verification_status != "verified":
        raise ProviderServiceError(
            "connection_not_verified",
            "Verify this provider connection before enabling the role.",
            409,
        )
    if payload.enabled:
        try:
            _vault().decrypt(
                connection.id,
                connection.encrypted_api_key,
                connection.api_key_nonce,
            )
        except CredentialVaultError as exc:
            raise _service_error_from_vault(exc) from None
    row = db.get(AssistantModelRole, role_id)
    if row is None:
        row = AssistantModelRole(
            role_id=role_id,
            connection_id=connection.id,
            model_id=payload.model_id,
        )
        db.add(row)
    row.connection_id = connection.id
    row.model_id = payload.model_id
    row.enabled = payload.enabled
    row.timeout_seconds = payload.timeout_seconds
    row.max_output_tokens = payload.max_output_tokens
    row.updated_at = utcnow()
    db.commit()
    db.refresh(row)
    return _role_out(role_id, row, connection, credential_available=True)


def delete_model_role(db: Session, role_id: str) -> None:
    if role_id not in MODEL_ROLE_BY_ID:
        raise ProviderServiceError("role_not_found", "Model role not found.", 404)
    row = db.get(AssistantModelRole, role_id)
    if row is not None:
        db.delete(row)
        db.commit()
