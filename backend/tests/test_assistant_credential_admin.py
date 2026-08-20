import base64
from collections.abc import Iterator
from pathlib import Path

import pytest
from fastapi.testclient import TestClient
from sqlalchemy import delete

from app.assistant.providers.credential_admin import (
    CredentialAdminError,
    audit_credentials,
    rotate_credentials,
)
from app.assistant.providers.credentials import CredentialVault, CredentialVaultError
from app.cli import main
from app.core.config import get_settings
from app.core.db import SessionLocal
from app.models.assistant_model_evaluation import AssistantModelEvaluation
from app.models.assistant_model_role import AssistantModelRole
from app.models.assistant_provider_connection import AssistantProviderConnection


@pytest.fixture(autouse=True)
def _clean_provider_configuration() -> Iterator[None]:
    def clean() -> None:
        with SessionLocal() as db:
            db.execute(delete(AssistantModelEvaluation))
            db.execute(delete(AssistantModelRole))
            db.execute(delete(AssistantProviderConnection))
            db.commit()

    clean()
    yield
    clean()


def _encoded_key(byte: int) -> str:
    return base64.urlsafe_b64encode(bytes([byte]) * 32).decode("ascii")


def _create_connection(client: TestClient, name: str, secret: str) -> str:
    response = client.post(
        "/api/assistant/providers/connections",
        json={
            "name": name,
            "adapter_id": "openai-compatible/v1",
            "base_url": "https://models.example.test/v1",
            "api_key": secret,
            "allow_private_network": False,
        },
    )
    assert response.status_code == 201, response.text
    return str(response.json()["id"])


def test_vault_key_id_is_stable_and_non_secret() -> None:
    first = CredentialVault.from_encoded_key(_encoded_key(1))
    same = CredentialVault.from_encoded_key(_encoded_key(1))
    different = CredentialVault.from_encoded_key(_encoded_key(2))

    assert first.key_id == same.key_id
    assert first.key_id != different.key_id
    assert len(first.key_id) == 16
    assert _encoded_key(1) not in first.key_id


def test_audit_distinguishes_saved_missing_and_unreadable_credentials(
    auth_client: TestClient,
) -> None:
    saved_id = _create_connection(auth_client, "Saved", "provider-secret-one")
    missing_id = _create_connection(auth_client, "Missing", "provider-secret-two")
    assert auth_client.delete(
        f"/api/assistant/providers/connections/{missing_id}/credential"
    ).status_code == 200
    current = CredentialVault.from_settings()

    with SessionLocal() as db:
        healthy = audit_credentials(db, current)
        saved = db.get(AssistantProviderConnection, saved_id)
        assert saved is not None
        saved.api_key_nonce = "broken"
        db.commit()
        broken = audit_credentials(db, current)

    assert healthy.total_connections == 2
    assert healthy.saved_credentials == 1
    assert healthy.connections_without_credentials == 1
    assert healthy.unreadable_credentials == 0
    assert healthy.healthy is True
    assert broken.unreadable_credentials == 1
    assert broken.healthy is False


def test_rotation_is_atomic_and_resets_dependent_gates(
    auth_client: TestClient,
) -> None:
    connection_id = _create_connection(
        auth_client,
        "Rotated",
        "provider-secret-to-preserve",
    )
    current = CredentialVault.from_settings()
    new = CredentialVault.from_encoded_key(_encoded_key(9))
    with SessionLocal() as db:
        connection = db.get(AssistantProviderConnection, connection_id)
        assert connection is not None
        connection.verification_status = "verified"
        connection.verified_models_json = '["eq-model"]'
        connection.verified_capabilities_json = '["structured-text/v1"]'
        role = AssistantModelRole(
            role_id="eq_assistant",
            connection_id=connection_id,
            model_id="eq-model",
            enabled=True,
            conformance_status="passed",
            conformance_fingerprint="f" * 64,
        )
        db.add(role)
        db.commit()
        old_ciphertext = connection.encrypted_api_key

        rotated = rotate_credentials(db, current, new)

    assert rotated == 1
    with SessionLocal() as db:
        connection = db.get(AssistantProviderConnection, connection_id)
        stored_role = db.get(AssistantModelRole, "eq_assistant")
        assert connection is not None
        assert stored_role is not None
        assert connection.encrypted_api_key != old_ciphertext
        assert new.decrypt(
            connection.id,
            connection.encrypted_api_key,
            connection.api_key_nonce,
        ) == "provider-secret-to-preserve"
        with pytest.raises(CredentialVaultError, match="credential_unreadable"):
            current.decrypt(
                connection.id,
                connection.encrypted_api_key,
                connection.api_key_nonce,
            )
        assert connection.verification_status == "never"
        assert connection.verified_models_json == "[]"
        assert connection.verified_capabilities_json == "[]"
        assert stored_role.enabled is True
        assert stored_role.conformance_status == "never"
        assert stored_role.conformance_fingerprint is None
        assert audit_credentials(db, new).healthy is True


def test_rotation_preflight_leaves_every_row_unchanged_on_corruption(
    auth_client: TestClient,
) -> None:
    first_id = _create_connection(auth_client, "First", "first-secret")
    second_id = _create_connection(auth_client, "Second", "second-secret")
    current = CredentialVault.from_settings()
    new = CredentialVault.from_encoded_key(_encoded_key(11))
    with SessionLocal() as db:
        first = db.get(AssistantProviderConnection, first_id)
        second = db.get(AssistantProviderConnection, second_id)
        assert first is not None and second is not None
        original_first = (first.encrypted_api_key, first.api_key_nonce)
        second.api_key_nonce = "broken"
        db.commit()

        with pytest.raises(CredentialAdminError, match="credential_unreadable"):
            rotate_credentials(db, current, new)
        db.rollback()

    with SessionLocal() as db:
        first = db.get(AssistantProviderConnection, first_id)
        assert first is not None
        assert (first.encrypted_api_key, first.api_key_nonce) == original_first


def test_cli_check_and_rotation_dry_run_are_read_only(
    auth_client: TestClient,
    monkeypatch: pytest.MonkeyPatch,
    capsys: pytest.CaptureFixture[str],
) -> None:
    connection_id = _create_connection(auth_client, "CLI", "cli-secret")
    with SessionLocal() as db:
        connection = db.get(AssistantProviderConnection, connection_id)
        assert connection is not None
        original = (connection.encrypted_api_key, connection.api_key_nonce)
    assert main(["assistant-credentials", "check"]) == 0
    checked = capsys.readouterr().out
    assert "unreadable credentials: 0" in checked
    assert "cli-secret" not in checked

    monkeypatch.setenv("ASSISTANT_CREDENTIAL_KEY_NEW", _encoded_key(15))
    assert main(["assistant-credentials", "rotate"]) == 0
    dry_run = capsys.readouterr().out
    assert "Dry run passed" in dry_run
    assert "cli-secret" not in dry_run
    with SessionLocal() as db:
        connection = db.get(AssistantProviderConnection, connection_id)
        assert connection is not None
        assert (connection.encrypted_api_key, connection.api_key_nonce) == original


def test_cli_rotation_requires_offline_acknowledgement(
    auth_client: TestClient,
    monkeypatch: pytest.MonkeyPatch,
    capsys: pytest.CaptureFixture[str],
) -> None:
    _create_connection(auth_client, "CLI", "cli-secret")
    monkeypatch.setenv("ASSISTANT_CREDENTIAL_KEY_NEW", _encoded_key(17))

    assert main(["assistant-credentials", "rotate", "--apply"]) == 2
    assert "--server-stopped" in capsys.readouterr().out


def test_cli_rotation_identifies_file_backed_key_for_replacement(
    auth_client: TestClient,
    monkeypatch: pytest.MonkeyPatch,
    capsys: pytest.CaptureFixture[str],
    tmp_path: Path,
) -> None:
    _create_connection(auth_client, "CLI file", "cli-secret")
    settings = get_settings()
    previous_key = settings.assistant_credential_key
    previous_file = settings.assistant_credential_key_file
    assert previous_key is not None
    key_file = tmp_path / "assistant-credential.key"
    key_file.write_text(previous_key.get_secret_value(), encoding="ascii")
    key_file.chmod(0o600)
    settings.assistant_credential_key = None
    settings.assistant_credential_key_file = key_file
    monkeypatch.setenv("ASSISTANT_CREDENTIAL_KEY_NEW", _encoded_key(19))
    try:
        result = main(
            [
                "assistant-credentials",
                "rotate",
                "--apply",
                "--server-stopped",
            ]
        )
    finally:
        settings.assistant_credential_key = previous_key
        settings.assistant_credential_key_file = previous_file

    output = capsys.readouterr().out
    assert result == 0
    assert f"replace the credential key file at {key_file}" in output
    assert "cli-secret" not in output
