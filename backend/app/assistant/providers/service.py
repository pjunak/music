from __future__ import annotations

import hashlib
import json
import secrets
from collections.abc import Iterable
from dataclasses import dataclass
from typing import Literal, cast

from sqlalchemy import delete, func, or_, select
from sqlalchemy.exc import IntegrityError
from sqlalchemy.orm import Session

from app.assistant.providers.credentials import (
    CredentialVault,
    CredentialVaultError,
    credential_storage_status,
    initialize_credential_storage,
    prepare_credential_storage_key_removal,
    remove_credential_storage_key_file,
)
from app.assistant.providers.definitions import (
    MODEL_ROLE_BY_ID,
    MODEL_ROLES,
    PROVIDER_ADAPTER_BY_ID,
    PROVIDER_ADAPTERS,
    PROVIDER_CAPABILITIES,
    PROVIDER_CAPABILITY_BY_ID,
    ModelRoleDefinition,
)
from app.assistant.providers.execution import (
    CONFORMANCE_CONTRACT,
    ProviderConformanceResult,
    ProviderExecutionTarget,
)
from app.assistant.providers.schemas import (
    ModelRoleDefinitionOut,
    ModelRoleOut,
    ModelRoleUpdate,
    ProviderAdapterOut,
    ProviderCapabilityOut,
    ProviderConnectionCreate,
    ProviderConnectionOut,
    ProviderConnectionUpdate,
    ProviderCredentialStorageResetOut,
    ProviderFrameworkStatusOut,
)
from app.assistant.providers.verification import (
    ProviderUrlError,
    ProviderVerificationResult,
    normalize_provider_base_url,
)
from app.core.config import get_settings
from app.jobs.service import ACTIVE_JOB_STATUSES
from app.models.assistant_model_evaluation import AssistantModelEvaluation
from app.models.assistant_model_role import AssistantModelRole
from app.models.assistant_provider_connection import AssistantProviderConnection
from app.models.background_job import BackgroundJob
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


@dataclass(frozen=True)
class ConformanceTarget:
    role_id: str
    execution: ProviderExecutionTarget
    challenge: str
    fingerprint: str


@dataclass(frozen=True)
class ResolvedRoleExecution:
    role_id: str
    execution: ProviderExecutionTarget
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
    if error.code.startswith("master_key_file_") or error.code.startswith(
        "master_key_directory_"
    ):
        return ProviderServiceError(
            error.code,
            "The server provider-credential key file is unavailable or unsafe.",
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


def _configurable_role(role_id: str) -> ModelRoleDefinition:
    definition = MODEL_ROLE_BY_ID.get(role_id)
    if definition is None:
        raise ProviderServiceError("role_not_found", "Model role not found.", 404)
    if not definition.configuration_available:
        raise ProviderServiceError(
            "role_not_available",
            "This model task is planned but is not available yet.",
            409,
        )
    return definition


def _capabilities_satisfy(
    available_ids: Iterable[str],
    required_ids: Iterable[str],
) -> bool:
    return set(required_ids).issubset(available_ids)


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


def _bounded_json_strings(value_json: str, *, limit: int) -> list[str]:
    try:
        value = json.loads(value_json)
    except json.JSONDecodeError:
        return []
    if not isinstance(value, list):
        return []
    result: list[str] = []
    for item in value:
        if (
            isinstance(item, str)
            and 0 < len(item) <= 256
            and item not in result
        ):
            result.append(item)
        if len(result) >= limit:
            break
    return result


def _models(row: AssistantProviderConnection) -> list[str]:
    return _bounded_json_strings(row.verified_models_json, limit=200)


def _verified_capability_ids(row: AssistantProviderConnection) -> list[str]:
    if row.verification_status != "verified":
        return []
    return [
        capability_id
        for capability_id in _bounded_json_strings(
            row.verified_capabilities_json,
            limit=len(PROVIDER_CAPABILITIES),
        )
        if capability_id in PROVIDER_CAPABILITY_BY_ID
    ]


def _verification_status(
    value: str,
) -> Literal["never", "verified", "failed"]:
    return cast(
        "Literal['never', 'verified', 'failed']",
        value if value in {"never", "verified", "failed"} else "failed",
    )


def _credential_saved(row: AssistantProviderConnection) -> bool:
    return bool(row.encrypted_api_key and row.api_key_nonce)


def _connection_credential(row: AssistantProviderConnection) -> str:
    if not _credential_saved(row):
        raise ProviderServiceError(
            "credential_missing",
            "Save an API key for this provider connection before using it.",
            409,
        )
    try:
        return _vault().decrypt(
            row.id,
            row.encrypted_api_key,
            row.api_key_nonce,
        )
    except CredentialVaultError as exc:
        raise _service_error_from_vault(exc) from None


def connection_out(row: AssistantProviderConnection) -> ProviderConnectionOut:
    credential_saved = _credential_saved(row)
    return ProviderConnectionOut(
        id=row.id,
        name=row.name,
        adapter_id=row.adapter_id,
        base_url=row.base_url,
        credential_saved=credential_saved,
        key_hint=row.api_key_hint if credential_saved else None,
        allow_private_network=row.allow_private_network,
        verification_status=_verification_status(row.verification_status),
        verification_error_code=row.verification_error_code,
        verified_models=_models(row),
        verified_capability_ids=_verified_capability_ids(row),
        last_verified_at=row.last_verified_at,
        created_at=row.created_at,
        updated_at=row.updated_at,
    )


def _saved_credentials_exist(db: Session) -> bool:
    return (
        db.scalar(
            select(AssistantProviderConnection.id)
            .where(
                or_(
                    AssistantProviderConnection.encrypted_api_key != "",
                    AssistantProviderConnection.api_key_nonce != "",
                )
            )
            .limit(1)
        )
        is not None
    )


def framework_status(db: Session) -> ProviderFrameworkStatusOut:
    storage = credential_storage_status(
        saved_credentials_exist=_saved_credentials_exist(db)
    )
    return ProviderFrameworkStatusOut(
        credential_storage_ready=storage.ready,
        credential_storage_error=storage.error,
        credential_storage_source=storage.source,
        credential_storage_key_id=storage.key_id,
        credential_storage_key_file_path=storage.key_file_path,
        credential_storage_host_directory_hint=(
            get_settings().assistant_credential_host_directory_hint
        ),
        credential_storage_can_initialize=storage.can_initialize,
        credential_storage_initialization_error=storage.initialization_error,
        capabilities=[
            ProviderCapabilityOut(
                id=capability.id,
                label=capability.label,
                description=capability.description,
            )
            for capability in PROVIDER_CAPABILITIES
        ],
        adapters=[
            ProviderAdapterOut(
                id=adapter.id,
                label=adapter.label,
                description=adapter.description,
                capability_ids=list(adapter.capability_ids),
            )
            for adapter in PROVIDER_ADAPTERS
        ],
        roles=[
            ModelRoleDefinitionOut(
                id=role.id,
                label=role.label,
                description=role.description,
                required_capability_ids=list(role.required_capability_ids),
                configuration_available=role.configuration_available,
            )
            for role in MODEL_ROLES
        ],
    )


def initialize_provider_credential_storage(
    db: Session,
) -> ProviderFrameworkStatusOut:
    try:
        initialize_credential_storage(
            saved_credentials_exist=_saved_credentials_exist(db)
        )
    except CredentialVaultError as exc:
        status_code = (
            409
            if exc.code
            in {
                "master_key_already_configured",
                "master_key_file_exists",
                "master_key_managed_by_environment",
                "saved_credentials_require_existing_key",
            }
            else 503
        )
        raise ProviderServiceError(
            exc.code,
            "Encrypted provider-key storage could not be initialized safely.",
            status_code,
        ) from None
    return framework_status(db)


def reset_provider_credential_storage(
    db: Session,
) -> ProviderCredentialStorageResetOut:
    """Erase provider secrets first, then remove the fixed file-backed key."""

    try:
        key_file = prepare_credential_storage_key_removal()
    except CredentialVaultError as exc:
        status_code = (
            409
            if exc.code
            in {
                "master_key_managed_by_environment",
                "master_key_file_not_configured",
            }
            else 503
        )
        raise ProviderServiceError(
            exc.code,
            "This credential store cannot be reset safely from the browser.",
            status_code,
        ) from None

    active_model_job = db.scalar(
        select(BackgroundJob.id)
        .where(
            BackgroundJob.kind.like("assistant.model%"),
            BackgroundJob.status.in_(ACTIVE_JOB_STATUSES),
        )
        .limit(1)
    )
    if active_model_job is not None:
        raise ProviderServiceError(
            "model_job_active",
            "Cancel or wait for active model jobs before resetting AI storage.",
            409,
        )

    connections = db.scalars(select(AssistantProviderConnection)).all()
    deleted_credentials = sum(_credential_saved(row) for row in connections)
    for row in connections:
        _clear_connection_credential(row)

    roles = db.scalars(select(AssistantModelRole)).all()
    for role in roles:
        _reset_role_conformance(role)
    db.execute(delete(AssistantModelEvaluation))
    db.commit()

    removal_error: str | None = None
    try:
        remove_credential_storage_key_file(key_file)
    except CredentialVaultError as exc:
        # The database no longer contains ciphertext that depends on this key.
        # Return the partial result so the UI does not misreport the credential
        # erasure as failed; the exact fixed-path removal can be retried.
        removal_error = exc.code

    return ProviderCredentialStorageResetOut(
        deleted_credentials=deleted_credentials,
        master_key_removed=removal_error is None,
        master_key_removal_error=removal_error,
        status=framework_status(db),
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
    if payload.api_key is not None and _credential_saved(row):
        raise ProviderServiceError(
            "credential_already_saved",
            "Delete the saved API key before adding a different one.",
            409,
        )
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
    if verification_inputs_changed:
        _require_no_active_model_jobs_for_connection(db, row.id)
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
        row.verified_capabilities_json = "[]"
        row.last_verified_at = None
        _reset_role_conformance_for_connection(db, row.id)
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


def delete_connection_credential(
    db: Session,
    connection_id: str,
) -> ProviderConnectionOut:
    row = get_connection(db, connection_id)
    _require_no_active_model_jobs_for_connection(db, row.id)
    _clear_connection_credential(row)
    _reset_role_conformance_for_connection(db, row.id)
    db.commit()
    db.refresh(row)
    return connection_out(row)


def _clear_connection_credential(
    row: AssistantProviderConnection,
) -> None:
    row.encrypted_api_key = ""
    row.api_key_nonce = ""
    row.api_key_hint = ""
    row.verification_status = "never"
    row.verification_error_code = None
    row.verified_models_json = "[]"
    row.verified_capabilities_json = "[]"
    row.last_verified_at = None
    row.updated_at = utcnow()


def _connection_fingerprint(row: AssistantProviderConnection) -> str:
    payload = "\0".join(
        (
            row.id,
            row.adapter_id,
            row.base_url,
            row.encrypted_api_key,
            row.api_key_nonce,
            row.verified_capabilities_json,
            "1" if row.allow_private_network else "0",
        )
    )
    return hashlib.sha256(payload.encode("utf-8")).hexdigest()


def _active_model_job_for_connection(
    db: Session,
    connection_id: str,
) -> BackgroundJob | None:
    role_ids = set(
        db.scalars(
            select(AssistantModelRole.role_id).where(
                AssistantModelRole.connection_id == connection_id
            )
        ).all()
    )
    if not role_ids:
        return None
    jobs = db.scalars(
        select(BackgroundJob).where(
            BackgroundJob.kind.like("assistant.model%"),
            BackgroundJob.status.in_(ACTIVE_JOB_STATUSES),
        )
    ).all()
    for job in jobs:
        try:
            parameters = json.loads(job.parameters_json)
        except (TypeError, json.JSONDecodeError):
            continue
        if isinstance(parameters, dict) and parameters.get("role_id") in role_ids:
            return job
    return None


def _require_no_active_model_jobs_for_connection(
    db: Session,
    connection_id: str,
) -> None:
    if _active_model_job_for_connection(db, connection_id) is not None:
        raise ProviderServiceError(
            "connection_model_job_active",
            "Wait for or cancel active model work before changing or verifying this connection.",
            409,
        )


def prepare_verification(db: Session, connection_id: str) -> VerificationTarget:
    row = get_connection(db, connection_id)
    _require_no_active_model_jobs_for_connection(db, row.id)
    api_key = _connection_credential(row)
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
    _require_no_active_model_jobs_for_connection(db, row.id)
    row.verification_status = "verified" if result.verified else "failed"
    row.verification_error_code = result.error_code
    row.verified_models_json = json.dumps(
        list(result.models) if result.verified else [],
        ensure_ascii=False,
        separators=(",", ":"),
    )
    adapter = PROVIDER_ADAPTER_BY_ID.get(row.adapter_id)
    supported_capabilities = set(adapter.capability_ids) if adapter is not None else set()
    verified_capability_ids = [
        capability_id
        for capability_id in result.capability_ids
        if capability_id in supported_capabilities
        and capability_id in PROVIDER_CAPABILITY_BY_ID
    ]
    row.verified_capabilities_json = json.dumps(
        verified_capability_ids if result.verified else [],
        ensure_ascii=False,
        separators=(",", ":"),
    )
    row.last_verified_at = utcnow()
    row.updated_at = row.last_verified_at
    _reset_role_conformance_for_connection(db, row.id)
    db.commit()
    db.refresh(row)
    return connection_out(row)


def _reset_role_conformance(row: AssistantModelRole) -> None:
    row.conformance_status = "never"
    row.conformance_error_code = None
    row.conformance_fingerprint = None
    row.last_conformance_at = None


def _reset_role_conformance_for_connection(db: Session, connection_id: str) -> None:
    roles = db.scalars(
        select(AssistantModelRole).where(
            AssistantModelRole.connection_id == connection_id
        )
    ).all()
    for role in roles:
        _reset_role_conformance(role)
    role_ids = [role.role_id for role in roles]
    if role_ids:
        db.execute(
            delete(AssistantModelEvaluation).where(
                AssistantModelEvaluation.role_id.in_(role_ids)
            )
        )


def _role_runtime_fingerprint(
    row: AssistantModelRole,
    connection: AssistantProviderConnection,
) -> str:
    payload = "\0".join(
        (
            CONFORMANCE_CONTRACT,
            _connection_fingerprint(connection),
            row.role_id,
            row.model_id,
            str(row.timeout_seconds),
            str(row.max_output_tokens),
        )
    )
    return hashlib.sha256(payload.encode("utf-8")).hexdigest()


def current_role_runtime_fingerprint(db: Session, role_id: str) -> str | None:
    """Return the current non-secret runtime identity without resolving credentials."""

    row = db.get(AssistantModelRole, role_id)
    if row is None:
        return None
    connection = db.get(AssistantProviderConnection, row.connection_id)
    if connection is None:
        return None
    return _role_runtime_fingerprint(row, connection)


def _conformance_status(
    row: AssistantModelRole | None,
    connection: AssistantProviderConnection | None,
) -> Literal["never", "passed", "failed"]:
    if row is None or connection is None:
        return "never"
    if row.conformance_fingerprint != _role_runtime_fingerprint(row, connection):
        return "never"
    if row.conformance_status in {"passed", "failed"}:
        return cast(
            "Literal['never', 'passed', 'failed']",
            row.conformance_status,
        )
    return "never"


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
    conformance_status = _conformance_status(row, connection)
    verified_capabilities = (
        set(_verified_capability_ids(connection)) if connection is not None else set()
    )
    capabilities_satisfied = _capabilities_satisfy(
        verified_capabilities,
        definition.required_capability_ids,
    )
    return ModelRoleOut(
        role_id=definition.id,
        label=definition.label,
        description=definition.description,
        required_capability_ids=list(definition.required_capability_ids),
        configuration_available=definition.configuration_available,
        connection_id=row.connection_id if row is not None else None,
        connection_name=connection.name if connection is not None else None,
        model_id=row.model_id if row is not None else "",
        enabled=row.enabled if row is not None else False,
        effective_enabled=(
            row is not None
            and row.enabled
            and definition.configuration_available
            and capabilities_satisfied
            and verification_status == "verified"
            and conformance_status == "passed"
            and credential_available
        ),
        timeout_seconds=row.timeout_seconds if row is not None else 30,
        max_output_tokens=row.max_output_tokens if row is not None else 2_000,
        verification_status=verification_status,
        conformance_status=conformance_status,
        conformance_error_code=(
            row.conformance_error_code
            if row is not None and conformance_status == "failed"
            else None
        ),
        last_conformance_at=(
            row.last_conformance_at
            if row is not None and conformance_status != "never"
            else None
        ),
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
            if not _credential_saved(connection):
                continue
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
    definition = _configurable_role(role_id)
    connection = get_connection(db, payload.connection_id)
    adapter = PROVIDER_ADAPTER_BY_ID.get(connection.adapter_id)
    supported_capabilities = set(adapter.capability_ids) if adapter is not None else set()
    if not _capabilities_satisfy(
        supported_capabilities,
        definition.required_capability_ids,
    ):
        raise ProviderServiceError(
            "incompatible_connection",
            "This connection type does not support the capabilities required by this task.",
            409,
        )
    if payload.enabled and connection.verification_status != "verified":
        raise ProviderServiceError(
            "connection_not_verified",
            "Verify this provider connection before enabling the role.",
            409,
        )
    if payload.enabled and not _capabilities_satisfy(
        _verified_capability_ids(connection),
        definition.required_capability_ids,
    ):
        raise ProviderServiceError(
            "incompatible_connection",
            "Verify a provider connection with the capabilities required by this task.",
            409,
        )
    if payload.enabled:
        _connection_credential(connection)
    row = db.get(AssistantModelRole, role_id)
    runtime_changed = (
        row is None
        or row.connection_id != connection.id
        or row.model_id != payload.model_id
        or row.timeout_seconds != payload.timeout_seconds
        or row.max_output_tokens != payload.max_output_tokens
    )
    if payload.enabled and (
        row is None
        or runtime_changed
        or _conformance_status(row, connection) != "passed"
    ):
        raise ProviderServiceError(
            "model_not_tested",
            "Save and test this model configuration before enabling it.",
            409,
        )
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
    if runtime_changed:
        _reset_role_conformance(row)
        db.execute(
            delete(AssistantModelEvaluation).where(
                AssistantModelEvaluation.role_id == role_id
            )
        )
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


def _execution_target(
    row: AssistantModelRole,
    connection: AssistantProviderConnection,
) -> ProviderExecutionTarget:
    api_key = _connection_credential(connection)
    return ProviderExecutionTarget(
        adapter_id=connection.adapter_id,
        base_url=connection.base_url,
        api_key=api_key,
        allow_private_network=connection.allow_private_network,
        model_id=row.model_id,
        timeout_seconds=row.timeout_seconds,
        max_output_tokens=row.max_output_tokens,
    )


def prepare_role_execution(db: Session, role_id: str) -> ProviderExecutionTarget:
    """Resolve one enabled, verified, conformant role for future feature code."""

    return prepare_role_execution_details(db, role_id).execution


def prepare_role_execution_details(
    db: Session,
    role_id: str,
) -> ResolvedRoleExecution:
    """Resolve execution plus the exact runtime fingerprint used by quality gates."""

    definition = _configurable_role(role_id)
    row = db.get(AssistantModelRole, role_id)
    if row is None or not row.enabled:
        raise ProviderServiceError(
            "role_not_enabled",
            "This model role is not enabled.",
            409,
        )
    connection = get_connection(db, row.connection_id)
    if connection.verification_status != "verified":
        raise ProviderServiceError(
            "connection_not_verified",
            "Verify this provider connection before using the role.",
            409,
        )
    if not _capabilities_satisfy(
        _verified_capability_ids(connection),
        definition.required_capability_ids,
    ):
        raise ProviderServiceError(
            "incompatible_connection",
            "The provider connection lacks a capability required by this task.",
            409,
        )
    if _conformance_status(row, connection) != "passed":
        raise ProviderServiceError(
            "model_not_tested",
            "Test this model configuration before using the role.",
            409,
        )
    return ResolvedRoleExecution(
        role_id=role_id,
        execution=_execution_target(row, connection),
        fingerprint=_role_runtime_fingerprint(row, connection),
    )


def prepare_role_conformance(db: Session, role_id: str) -> ConformanceTarget:
    definition = _configurable_role(role_id)
    row = db.get(AssistantModelRole, role_id)
    if row is None:
        raise ProviderServiceError(
            "role_not_configured",
            "Save a model configuration before testing it.",
            409,
        )
    connection = get_connection(db, row.connection_id)
    if connection.verification_status != "verified":
        raise ProviderServiceError(
            "connection_not_verified",
            "Verify this provider connection before testing the model.",
            409,
        )
    if not _capabilities_satisfy(
        _verified_capability_ids(connection),
        definition.required_capability_ids,
    ):
        raise ProviderServiceError(
            "incompatible_connection",
            "The provider connection lacks a capability required by this task.",
            409,
        )
    return ConformanceTarget(
        role_id=role_id,
        execution=_execution_target(row, connection),
        challenge=secrets.token_urlsafe(24),
        fingerprint=_role_runtime_fingerprint(row, connection),
    )


def finish_role_conformance(
    db: Session,
    target: ConformanceTarget,
    result: ProviderConformanceResult,
) -> ModelRoleOut:
    db.expire_all()
    row = db.get(AssistantModelRole, target.role_id)
    if row is None:
        raise ProviderServiceError(
            "role_changed",
            "The model role changed while its test was running. Test it again.",
            409,
        )
    connection = get_connection(db, row.connection_id)
    if _role_runtime_fingerprint(row, connection) != target.fingerprint:
        raise ProviderServiceError(
            "role_changed",
            "The model role changed while its test was running. Test it again.",
            409,
        )
    row.conformance_status = "passed" if result.passed else "failed"
    row.conformance_error_code = result.error_code
    row.conformance_fingerprint = target.fingerprint
    row.last_conformance_at = utcnow()
    row.updated_at = row.last_conformance_at
    db.commit()
    db.refresh(row)
    return _role_out(
        target.role_id,
        row,
        connection,
        credential_available=True,
    )
