from __future__ import annotations

import base64
import binascii
import hashlib
import os
from dataclasses import dataclass

from cryptography.exceptions import InvalidTag
from cryptography.hazmat.primitives.ciphers.aead import AESGCM

from app.core.config import get_settings

_AAD_PREFIX = b"assistant-provider-credential/v1:"
_NONCE_BYTES = 12


class CredentialVaultError(RuntimeError):
    def __init__(self, code: str) -> None:
        super().__init__(code)
        self.code = code


@dataclass(frozen=True)
class EncryptedCredential:
    ciphertext: str
    nonce: str
    hint: str


def _decode_master_key(value: str) -> bytes:
    try:
        key = base64.b64decode(value.strip(), altchars=b"-_", validate=True)
    except (ValueError, binascii.Error) as exc:
        raise CredentialVaultError("invalid_master_key") from exc
    if len(key) != 32:
        raise CredentialVaultError("invalid_master_key")
    return key


class CredentialVault:
    def __init__(self, key: bytes) -> None:
        if len(key) != 32:
            raise CredentialVaultError("invalid_master_key")
        self._cipher = AESGCM(key)
        self._key_id = hashlib.sha256(key).hexdigest()[:16]

    @classmethod
    def from_settings(cls) -> CredentialVault:
        configured = get_settings().assistant_credential_key
        if configured is None or not configured.get_secret_value().strip():
            raise CredentialVaultError("master_key_not_configured")
        return cls(_decode_master_key(configured.get_secret_value()))

    @classmethod
    def from_encoded_key(cls, value: str) -> CredentialVault:
        """Build a vault from an operator-supplied encoded key without changing settings."""

        return cls(_decode_master_key(value))

    @property
    def key_id(self) -> str:
        """Non-secret fingerprint suitable for pairing a key with a database backup."""

        return self._key_id

    @staticmethod
    def _aad(connection_id: str) -> bytes:
        return _AAD_PREFIX + connection_id.encode("ascii")

    def encrypt(self, connection_id: str, api_key: str) -> EncryptedCredential:
        secret = api_key.strip()
        if not secret:
            raise CredentialVaultError("empty_credential")
        nonce = os.urandom(_NONCE_BYTES)
        encrypted = self._cipher.encrypt(
            nonce,
            secret.encode("utf-8"),
            self._aad(connection_id),
        )
        return EncryptedCredential(
            ciphertext=base64.urlsafe_b64encode(encrypted).decode("ascii"),
            nonce=base64.urlsafe_b64encode(nonce).decode("ascii"),
            hint=f"••••{secret[-4:] if len(secret) > 4 else ''}",
        )

    def decrypt(self, connection_id: str, ciphertext: str, nonce: str) -> str:
        try:
            encrypted = base64.b64decode(
                ciphertext, altchars=b"-_", validate=True
            )
            nonce_bytes = base64.b64decode(nonce, altchars=b"-_", validate=True)
            if len(nonce_bytes) != _NONCE_BYTES:
                raise ValueError("invalid nonce length")
            cleartext = self._cipher.decrypt(
                nonce_bytes,
                encrypted,
                self._aad(connection_id),
            )
            return cleartext.decode("utf-8")
        except (InvalidTag, UnicodeDecodeError, ValueError, binascii.Error) as exc:
            raise CredentialVaultError("credential_unreadable") from exc


def credential_vault_status() -> tuple[bool, str | None]:
    try:
        CredentialVault.from_settings()
    except CredentialVaultError as exc:
        return False, exc.code
    return True, None
