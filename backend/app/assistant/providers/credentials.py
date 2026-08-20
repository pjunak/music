from __future__ import annotations

import base64
import binascii
import hashlib
import os
import stat
from contextlib import suppress
from dataclasses import dataclass
from pathlib import Path
from typing import Literal

from cryptography.exceptions import InvalidTag
from cryptography.hazmat.primitives.ciphers.aead import AESGCM

from app.core.config import get_settings

_AAD_PREFIX = b"assistant-provider-credential/v1:"
_NONCE_BYTES = 12
_MASTER_KEY_BYTES = 32
_MAX_ENCODED_KEY_BYTES = 128


class CredentialVaultError(RuntimeError):
    def __init__(self, code: str) -> None:
        super().__init__(code)
        self.code = code


@dataclass(frozen=True)
class EncryptedCredential:
    ciphertext: str
    nonce: str
    hint: str


CredentialStorageSource = Literal["environment", "file"]


@dataclass(frozen=True)
class CredentialStorageStatus:
    ready: bool
    error: str | None
    source: CredentialStorageSource | None
    key_id: str | None
    key_file_path: str | None
    can_initialize: bool
    initialization_error: str | None


def _decode_master_key(value: str) -> bytes:
    try:
        key = base64.b64decode(value.strip(), altchars=b"-_", validate=True)
    except (ValueError, binascii.Error) as exc:
        raise CredentialVaultError("invalid_master_key") from exc
    if len(key) != _MASTER_KEY_BYTES:
        raise CredentialVaultError("invalid_master_key")
    return key


def _environment_key() -> str | None:
    configured = get_settings().assistant_credential_key
    if configured is None:
        return None
    value = configured.get_secret_value().strip()
    return value or None


def _key_file() -> Path | None:
    return get_settings().assistant_credential_key_file


def _read_key_file(path: Path) -> str:
    if path.is_symlink():
        raise CredentialVaultError("master_key_file_unsafe")
    flags = os.O_RDONLY | getattr(os, "O_NOFOLLOW", 0)
    try:
        file_descriptor = os.open(path, flags)
    except FileNotFoundError as exc:
        raise CredentialVaultError("master_key_not_configured") from exc
    except OSError as exc:
        raise CredentialVaultError("master_key_file_unreadable") from exc
    try:
        file_stat = os.fstat(file_descriptor)
        if not stat.S_ISREG(file_stat.st_mode):
            raise CredentialVaultError("master_key_file_unsafe")
        if os.name == "posix" and stat.S_IMODE(file_stat.st_mode) & 0o077:
            raise CredentialVaultError("master_key_file_permissions")
        chunks: list[bytes] = []
        remaining = _MAX_ENCODED_KEY_BYTES + 1
        while remaining:
            chunk = os.read(file_descriptor, remaining)
            if not chunk:
                break
            chunks.append(chunk)
            remaining -= len(chunk)
        raw = b"".join(chunks)
        if len(raw) > _MAX_ENCODED_KEY_BYTES:
            raise CredentialVaultError("invalid_master_key")
    finally:
        os.close(file_descriptor)
    try:
        return raw.decode("ascii").strip()
    except UnicodeDecodeError as exc:
        raise CredentialVaultError("invalid_master_key") from exc


def _configured_key() -> tuple[str, CredentialStorageSource]:
    environment_key = _environment_key()
    if environment_key is not None:
        return environment_key, "environment"
    key_file = _key_file()
    if key_file is None:
        raise CredentialVaultError("master_key_not_configured")
    return _read_key_file(key_file), "file"


def _initialization_target_error(path: Path) -> str | None:
    if not path.is_absolute():
        return "master_key_file_path_not_absolute"
    if os.path.lexists(path):
        return "master_key_file_exists"
    parent = path.parent
    try:
        parent_stat = parent.lstat()
    except OSError:
        return "master_key_directory_unavailable"
    if parent.is_symlink() or not stat.S_ISDIR(parent_stat.st_mode):
        return "master_key_directory_unsafe"
    if os.name == "posix" and stat.S_IMODE(parent_stat.st_mode) & 0o077:
        return "master_key_directory_permissions"
    if not os.access(parent, os.W_OK | os.X_OK):
        return "master_key_directory_not_writable"
    return None


def credential_storage_status(
    *,
    saved_credentials_exist: bool = False,
) -> CredentialStorageStatus:
    key_file = _key_file()
    key_file_path = str(key_file) if key_file is not None else None
    environment_key = _environment_key()
    source: CredentialStorageSource | None = None
    if environment_key is not None:
        source = "environment"
    elif key_file is not None and os.path.lexists(key_file):
        source = "file"
    error: str | None
    key_id: str | None
    try:
        configured, resolved_source = _configured_key()
        vault = CredentialVault.from_encoded_key(configured)
    except CredentialVaultError as exc:
        ready = False
        error = exc.code
        key_id = None
    else:
        ready = True
        error = None
        key_id = vault.key_id
        source = resolved_source

    initialization_error: str | None
    if ready:
        initialization_error = "master_key_already_configured"
    elif environment_key is not None:
        initialization_error = "master_key_managed_by_environment"
    elif key_file is None:
        initialization_error = "master_key_file_not_configured"
    elif os.path.lexists(key_file):
        initialization_error = "master_key_file_exists"
    elif saved_credentials_exist:
        initialization_error = "saved_credentials_require_existing_key"
    else:
        initialization_error = _initialization_target_error(key_file)

    return CredentialStorageStatus(
        ready=ready,
        error=error,
        source=source,
        key_id=key_id,
        key_file_path=key_file_path,
        can_initialize=initialization_error is None,
        initialization_error=initialization_error,
    )


def initialize_credential_storage(
    *,
    saved_credentials_exist: bool = False,
) -> CredentialStorageStatus:
    current = credential_storage_status(
        saved_credentials_exist=saved_credentials_exist
    )
    if not current.can_initialize:
        raise CredentialVaultError(
            current.initialization_error or "master_key_initialization_unavailable"
        )
    key_file = _key_file()
    if key_file is None:  # guarded by status; keeps the write path explicit
        raise CredentialVaultError("master_key_file_not_configured")
    encoded = base64.urlsafe_b64encode(os.urandom(_MASTER_KEY_BYTES))
    flags = os.O_WRONLY | os.O_CREAT | os.O_EXCL | getattr(os, "O_NOFOLLOW", 0)
    file_descriptor: int | None = None
    remove_partial_file = False
    try:
        file_descriptor = os.open(key_file, flags, 0o600)
        written = 0
        while written < len(encoded):
            chunk_size = os.write(file_descriptor, encoded[written:])
            if chunk_size <= 0:
                raise OSError("credential key write made no progress")
            written += chunk_size
        if os.name == "posix":
            os.chmod(key_file, 0o600)
        os.fsync(file_descriptor)
    except FileExistsError as exc:
        raise CredentialVaultError("master_key_file_exists") from exc
    except OSError as exc:
        if file_descriptor is not None:
            remove_partial_file = True
        raise CredentialVaultError("master_key_file_write_failed") from exc
    finally:
        if file_descriptor is not None:
            os.close(file_descriptor)
        if remove_partial_file:
            with suppress(OSError):
                os.unlink(key_file)
    initialized = credential_storage_status(saved_credentials_exist=False)
    if not initialized.ready:
        raise CredentialVaultError(
            initialized.error or "master_key_initialization_failed"
        )
    return initialized


class CredentialVault:
    def __init__(self, key: bytes) -> None:
        if len(key) != _MASTER_KEY_BYTES:
            raise CredentialVaultError("invalid_master_key")
        self._cipher = AESGCM(key)
        self._key_id = hashlib.sha256(key).hexdigest()[:16]

    @classmethod
    def from_settings(cls) -> CredentialVault:
        configured, _source = _configured_key()
        return cls(_decode_master_key(configured))

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
    status = credential_storage_status()
    return status.ready, status.error
