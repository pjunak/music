"""Content digests for executable Assistant model-role contracts."""

from __future__ import annotations

import hashlib
from pathlib import Path

_ASSISTANT_DIR = Path(__file__).resolve().parent.parent
_COMMON_FILES = (
    _ASSISTANT_DIR / "structured_harness.py",
    _ASSISTANT_DIR / "providers" / "execution.py",
)
_ROLE_FILES: dict[str, tuple[Path, ...]] = {
    "playlist_planner": (
        _ASSISTANT_DIR / "local.py",
        _ASSISTANT_DIR / "model_playlist.py",
        _ASSISTANT_DIR / "evaluation.py",
        _ASSISTANT_DIR / "evaluation_suites" / "playlist-local-v1.json",
        _ASSISTANT_DIR / "evaluation_suites" / "playlist-model-v1.json",
    ),
    "music_tagger": (
        _ASSISTANT_DIR / "model_tagger.py",
        _ASSISTANT_DIR / "library_context.py",
        _ASSISTANT_DIR / "tag_vocabulary.py",
        _ASSISTANT_DIR / "evaluation_suites" / "music-tagging-v1.json",
    ),
    "tag_cleanup": (
        _ASSISTANT_DIR / "model_tag_cleanup.py",
        _ASSISTANT_DIR / "tag_cleanup.py",
        _ASSISTANT_DIR / "tag_vocabulary.py",
        _ASSISTANT_DIR / "evaluation_suites" / "tag-cleanup-v1.json",
    ),
    "eq_assistant": (
        _ASSISTANT_DIR / "model_eq.py",
        _ASSISTANT_DIR / "evaluation_suites" / "eq-assistant-v1.json",
    ),
    "library_cleanup": (),
    "audio_analyzer": (),
}


def role_executable_contract_digest(role_id: str) -> str:
    """Hash the checked-in code and suites that define one role's behavior."""

    try:
        role_files = _ROLE_FILES[role_id]
    except KeyError as exc:
        raise ValueError(f"unknown Assistant model role: {role_id}") from exc

    digest = hashlib.sha256()
    for path in (*_COMMON_FILES, *role_files):
        relative = path.relative_to(_ASSISTANT_DIR).as_posix().encode("utf-8")
        digest.update(len(relative).to_bytes(4, "big"))
        digest.update(relative)
        contents = path.read_bytes()
        digest.update(len(contents).to_bytes(8, "big"))
        digest.update(contents)
    return digest.hexdigest()


def fingerprinted_role_ids() -> frozenset[str]:
    return frozenset(_ROLE_FILES)
