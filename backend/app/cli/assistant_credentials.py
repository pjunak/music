"""Offline provider-credential audit and master-key rotation commands."""

from __future__ import annotations

import argparse
import os

from app.assistant.providers.credential_admin import (
    CredentialAdminError,
    audit_credentials,
    rotate_credentials,
)
from app.assistant.providers.credentials import (
    CredentialVault,
    CredentialVaultError,
    credential_storage_status,
)
from app.core.db import SessionLocal

_NEW_KEY_ENV = "ASSISTANT_CREDENTIAL_KEY_NEW"


def add_parser(sub: argparse._SubParsersAction) -> None:
    parser = sub.add_parser(
        "assistant-credentials",
        help="Audit restored provider credentials or rotate their encryption key offline",
    )
    actions = parser.add_subparsers(dest="credential_action", required=True)

    check = actions.add_parser(
        "check",
        help="Verify that the configured key decrypts every saved provider credential",
    )
    check.set_defaults(handler=run)

    rotate = actions.add_parser(
        "rotate",
        help=(
            f"Validate and optionally rotate to the key in {_NEW_KEY_ENV}; "
            "defaults to a read-only dry run"
        ),
    )
    rotate.add_argument(
        "--apply",
        action="store_true",
        help="Commit the re-encryption after all credentials pass preflight",
    )
    rotate.add_argument(
        "--server-stopped",
        action="store_true",
        help="Acknowledge that no Music server process is using this database",
    )
    rotate.set_defaults(handler=run)


def _current_vault() -> CredentialVault | None:
    try:
        return CredentialVault.from_settings()
    except CredentialVaultError as exc:
        print(f"Credential key is not usable ({exc.code}).")
        return None


def _print_audit(vault: CredentialVault) -> bool:
    with SessionLocal() as db:
        report = audit_credentials(db, vault)
    print(f"key id: {vault.key_id}")
    print(f"connections: {report.total_connections}")
    print(f"saved credentials: {report.saved_credentials}")
    print(f"connections without a credential: {report.connections_without_credentials}")
    print(f"unreadable credentials: {report.unreadable_credentials}")
    return report.healthy


def _run_rotate(args: argparse.Namespace, current: CredentialVault) -> int:
    encoded_new_key = os.environ.get(_NEW_KEY_ENV, "")
    if not encoded_new_key.strip():
        print(f"Set {_NEW_KEY_ENV} to a new URL-safe base64 32-byte key first.")
        return 2
    try:
        new = CredentialVault.from_encoded_key(encoded_new_key)
    except CredentialVaultError as exc:
        print(f"New credential key is not usable ({exc.code}).")
        return 2
    if current.key_id == new.key_id:
        print("The new key is the same as the current key.")
        return 2
    if not _print_audit(current):
        print("Rotation stopped: the current key cannot decrypt every saved credential.")
        return 2
    print(f"new key id: {new.key_id}")
    if not args.apply:
        print("Dry run passed. No database rows were changed.")
        print("Stop the Music server, then rerun with --apply --server-stopped.")
        return 0
    if not args.server_stopped:
        print("Refusing to rotate without --server-stopped.")
        return 2
    try:
        with SessionLocal() as db:
            rotated = rotate_credentials(db, current, new)
    except CredentialAdminError as exc:
        print(f"Rotation failed before commit ({exc.code}).")
        return 2
    print(f"Rotated {rotated} saved provider credential(s) atomically.")
    storage = credential_storage_status()
    if storage.source == "file" and storage.key_file_path:
        print(
            "Before restarting, replace the credential key file at "
            f"{storage.key_file_path} with the key whose id is {new.key_id}."
        )
    else:
        print(
            "Before restarting, replace ASSISTANT_CREDENTIAL_KEY with the key "
            f"whose id is {new.key_id}."
        )
    print("Provider connections must be verified and model quality gates rerun.")
    return 0


def run(args: argparse.Namespace) -> int:
    current = _current_vault()
    if current is None:
        return 2
    if args.credential_action == "check":
        return 0 if _print_audit(current) else 2
    if args.credential_action == "rotate":
        return _run_rotate(args, current)
    raise AssertionError(f"unknown assistant credential action: {args.credential_action}")
