import json
import sqlite3

from app.reference_contracts import REFERENCE_DIR, reference_drift


def test_rewrite_reference_contracts_are_current() -> None:
    assert reference_drift(REFERENCE_DIR) == []


def test_synthetic_crypto_fixture_matches_python() -> None:
    from app.assistant.providers.credentials import CredentialVault
    from app.core.security import verify_password

    compatibility = json.loads(
        (REFERENCE_DIR / "compatibility-data.json").read_text(encoding="utf-8")
    )
    password = compatibility["argon2id"]
    assert verify_password(password["phc"], password["password"])
    assert not verify_password(password["phc"], password["invalid_password"])

    credential = compatibility["aes_256_gcm"]
    vault = CredentialVault.from_encoded_key(credential["key_urlsafe_base64"])
    assert vault.key_id == credential["key_id"]
    assert (
        vault.decrypt(
            credential["connection_id"],
            credential["ciphertext_urlsafe_base64"],
            credential["nonce_urlsafe_base64"],
        )
        == credential["plaintext"]
    )


def test_synthetic_sqlite_fixture_populates_every_table() -> None:
    compatibility = json.loads(
        (REFERENCE_DIR / "compatibility-data.json").read_text(encoding="utf-8")
    )
    database = sqlite3.connect(":memory:")
    try:
        database.execute("PRAGMA foreign_keys=ON")
        database.executescript(
            (REFERENCE_DIR / "sqlite-fixture.sql").read_text(encoding="utf-8")
        )
        tables = [
            row[0]
            for row in database.execute(
                "SELECT name FROM sqlite_master "
                "WHERE type = 'table' AND name NOT LIKE 'sqlite_%' ORDER BY name"
            )
        ]
        assert len(tables) == compatibility["sqlite"]["table_count"]
        for table in tables:
            row_count = database.execute(f'SELECT COUNT(*) FROM "{table}"').fetchone()
            assert row_count == (compatibility["sqlite"]["representative_rows_per_table"],)
        assert database.execute("PRAGMA foreign_key_check").fetchall() == []

        stored = database.execute(
            "SELECT password_hash, created_at FROM users WHERE id = 1"
        ).fetchone()
        assert stored == (
            compatibility["argon2id"]["phc"],
            compatibility["sqlite"]["timestamp"],
        )
    finally:
        database.close()


def test_legacy_device_fixture_matches_python(tmp_path, monkeypatch) -> None:
    from app.core.config import get_settings
    from app.devices.store import DeviceStore

    compatibility = json.loads(
        (REFERENCE_DIR / "compatibility-data.json").read_text(encoding="utf-8")
    )
    devices_path = tmp_path / "devices.json"
    with monkeypatch.context() as scoped:
        scoped.setenv("DEVICES_FILE", str(devices_path))
        get_settings.cache_clear()
        for case in compatibility["legacy_device_cases"]:
            devices_path.unlink(missing_ok=True)
            if case["source"] is not None:
                devices_path.write_text(case["source"], encoding="utf-8")
            store = DeviceStore()
            store.load()
            assert store.list() == case["expected"], case["id"]
    get_settings.cache_clear()
