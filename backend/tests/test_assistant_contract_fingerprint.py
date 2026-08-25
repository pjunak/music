from pathlib import Path

from app.assistant.providers.contract_fingerprint import (
    fingerprinted_role_ids,
    role_executable_contract_digest,
)


def test_every_model_role_has_an_executable_contract_digest() -> None:
    assert fingerprinted_role_ids() == {
        "playlist_planner",
        "music_tagger",
        "tag_cleanup",
        "eq_assistant",
        "library_cleanup",
        "audio_analyzer",
    }
    assert all(
        len(role_executable_contract_digest(role_id)) == 64
        for role_id in fingerprinted_role_ids()
    )


def test_role_digest_changes_only_for_consumed_source_bytes(
    monkeypatch,
) -> None:
    eq_before = role_executable_contract_digest("eq_assistant")
    playlist_before = role_executable_contract_digest("playlist_planner")
    original_read_bytes = Path.read_bytes

    def changed_eq_source(path: Path) -> bytes:
        contents = original_read_bytes(path)
        return contents + b"\nchanged" if path.name == "model_eq.py" else contents

    monkeypatch.setattr(Path, "read_bytes", changed_eq_source)

    assert role_executable_contract_digest("eq_assistant") != eq_before
    assert role_executable_contract_digest("playlist_planner") == playlist_before
