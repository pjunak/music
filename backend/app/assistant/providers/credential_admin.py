"""Offline audit and atomic master-key rotation for provider credentials."""

from __future__ import annotations

from dataclasses import dataclass

from sqlalchemy import delete, select
from sqlalchemy.orm import Session

from app.assistant.providers.credentials import CredentialVault, CredentialVaultError
from app.models.assistant_model_evaluation import AssistantModelEvaluation
from app.models.assistant_model_role import AssistantModelRole
from app.models.assistant_provider_connection import AssistantProviderConnection
from app.models.base import utcnow


class CredentialAdminError(RuntimeError):
    def __init__(self, code: str) -> None:
        super().__init__(code)
        self.code = code


@dataclass(frozen=True)
class CredentialAudit:
    total_connections: int
    saved_credentials: int
    connections_without_credentials: int
    unreadable_credentials: int

    @property
    def healthy(self) -> bool:
        return self.unreadable_credentials == 0


def _credential_state(row: AssistantProviderConnection) -> str:
    has_ciphertext = bool(row.encrypted_api_key)
    has_nonce = bool(row.api_key_nonce)
    if has_ciphertext and has_nonce:
        return "saved"
    if not has_ciphertext and not has_nonce:
        return "missing"
    return "invalid"


def audit_credentials(db: Session, vault: CredentialVault) -> CredentialAudit:
    rows = list(
        db.scalars(select(AssistantProviderConnection).order_by(AssistantProviderConnection.id))
    )
    saved = 0
    missing = 0
    unreadable = 0
    for row in rows:
        state = _credential_state(row)
        if state == "missing":
            missing += 1
            continue
        if state == "invalid":
            unreadable += 1
            continue
        saved += 1
        try:
            vault.decrypt(row.id, row.encrypted_api_key, row.api_key_nonce)
        except CredentialVaultError:
            unreadable += 1
    return CredentialAudit(
        total_connections=len(rows),
        saved_credentials=saved,
        connections_without_credentials=missing,
        unreadable_credentials=unreadable,
    )


def rotate_credentials(
    db: Session,
    current_vault: CredentialVault,
    new_vault: CredentialVault,
) -> int:
    """Re-encrypt every saved credential and reset dependent gates in one commit."""

    if current_vault.key_id == new_vault.key_id:
        raise CredentialAdminError("same_master_key")
    rows = list(
        db.scalars(select(AssistantProviderConnection).order_by(AssistantProviderConnection.id))
    )
    cleartext: dict[str, str] = {}
    for row in rows:
        state = _credential_state(row)
        if state == "missing":
            continue
        if state == "invalid":
            raise CredentialAdminError("credential_unreadable")
        try:
            cleartext[row.id] = current_vault.decrypt(
                row.id,
                row.encrypted_api_key,
                row.api_key_nonce,
            )
        except CredentialVaultError as exc:
            raise CredentialAdminError("credential_unreadable") from exc

    affected_connection_ids = set(cleartext)
    now = utcnow()
    for row in rows:
        secret = cleartext.get(row.id)
        if secret is None:
            continue
        encrypted = new_vault.encrypt(row.id, secret)
        row.encrypted_api_key = encrypted.ciphertext
        row.api_key_nonce = encrypted.nonce
        row.api_key_hint = encrypted.hint
        row.verification_status = "never"
        row.verification_error_code = None
        row.verified_models_json = "[]"
        row.verified_capabilities_json = "[]"
        row.last_verified_at = None
        row.updated_at = now

    if affected_connection_ids:
        roles = list(
            db.scalars(
                select(AssistantModelRole).where(
                    AssistantModelRole.connection_id.in_(affected_connection_ids)
                )
            )
        )
        affected_role_ids = {role.role_id for role in roles}
        for role in roles:
            role.conformance_status = "never"
            role.conformance_error_code = None
            role.conformance_fingerprint = None
            role.last_conformance_at = None
            role.updated_at = now
        if affected_role_ids:
            db.execute(
                delete(AssistantModelEvaluation).where(
                    AssistantModelEvaluation.role_id.in_(affected_role_ids)
                )
            )
    db.commit()
    return len(cleartext)
