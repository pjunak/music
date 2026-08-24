from __future__ import annotations

import re
from pathlib import Path
from urllib.parse import unquote

from app.assistant.model_eq import (
    EQ_DRAFT_ENGINE_ID,
    EQ_DRAFT_INPUT_CONTRACT,
    EQ_DRAFT_OUTPUT_CONTRACT,
)
from app.assistant.model_evaluation import (
    EQ_QUALITY_EVALUATION_ID,
    PLAYLIST_QUALITY_EVALUATION_ID,
    TAG_CLEANUP_QUALITY_EVALUATION_ID,
    TAGGING_QUALITY_EVALUATION_ID,
)
from app.assistant.model_playlist import (
    MODEL_PLAYLIST_INPUT_CONTRACT,
    MODEL_PLAYLIST_OUTPUT_CONTRACT,
)
from app.assistant.model_tag_cleanup import (
    MODEL_TAG_CLEANUP_ENGINE_ID,
    MODEL_TAG_CLEANUP_INPUT_CONTRACT,
    MODEL_TAG_CLEANUP_OUTPUT_CONTRACT,
)
from app.assistant.model_tagger import (
    MODEL_TAG_ANALYZER_ID,
    MODEL_TAGGER_INPUT_CONTRACT,
    MODEL_TAGGER_OUTPUT_CONTRACT,
)
from app.assistant.providers.definitions import MODEL_ROLE_RUNTIME_CONTRACTS
from app.assistant.providers.execution import CONFORMANCE_CONTRACT
from app.assistant.schemas import (
    MODEL_EQ_DISCLOSURE_VERSION,
    MODEL_PLAYLIST_DISCLOSURE_VERSION,
)
from app.assistant.structured_harness import STRUCTURED_HARNESS_CONTRACT
from app.assistant.tag_schemas import (
    MODEL_TAG_CLEANUP_DISCLOSURE_VERSION,
    MODEL_TAGGING_DISCLOSURE_VERSION,
)

REPOSITORY_ROOT = Path(__file__).resolve().parents[2]
DOCS_ROOT = REPOSITORY_ROOT / "docs"
ASSISTANT_ARCHITECTURE = DOCS_ROOT / "ASSISTANT_ARCHITECTURE.md"
_MARKDOWN_LINK = re.compile(r"!?(?:\[[^\]]*\])\(([^)]+)\)")
_PRIVATE_DOC_NAMES = {"BACKGROUND_JOBS.md", "FUTURE.md", "ui-redesign-plan.md"}


def _maintained_markdown_files() -> tuple[Path, ...]:
    files = [
        *REPOSITORY_ROOT.glob("*.md"),
        *DOCS_ROOT.glob("*.md"),
        *(REPOSITORY_ROOT / "clients").rglob("*.md"),
        *(REPOSITORY_ROOT / "backend" / "evaluation").glob("*.md"),
    ]
    return tuple(
        sorted(
            path
            for path in set(files)
            if not (path.parent == DOCS_ROOT and path.name in _PRIVATE_DOC_NAMES)
        )
    )


def _link_target(raw_target: str) -> str:
    raw_target = raw_target.strip()
    if raw_target.startswith("<") and ">" in raw_target:
        return raw_target[1 : raw_target.index(">")]
    return raw_target.split(maxsplit=1)[0]


def test_local_markdown_links_resolve() -> None:
    missing: list[str] = []
    for document in _maintained_markdown_files():
        for match in _MARKDOWN_LINK.finditer(document.read_text(encoding="utf-8")):
            target = _link_target(match.group(1))
            if target.startswith(("#", "http://", "https://", "mailto:")):
                continue
            local_target = unquote(target.split("#", 1)[0].split("?", 1)[0])
            if local_target and not (document.parent / local_target).exists():
                missing.append(f"{document.relative_to(REPOSITORY_ROOT)} -> {local_target}")
    assert not missing, "Missing local documentation targets:\n" + "\n".join(missing)


def test_documentation_index_covers_maintained_docs() -> None:
    index = (DOCS_ROOT / "README.md").read_text(encoding="utf-8")
    excluded = {"README.md", *_PRIVATE_DOC_NAMES}
    missing = [
        path.name
        for path in DOCS_ROOT.glob("*.md")
        if path.name not in excluded and f"({path.name}" not in index
    ]
    assert not missing, "Documentation index is missing: " + ", ".join(sorted(missing))


def test_assistant_contract_inventory_matches_runtime() -> None:
    inventory = ASSISTANT_ARCHITECTURE.read_text(encoding="utf-8")
    contract_values = {
        STRUCTURED_HARNESS_CONTRACT,
        CONFORMANCE_CONTRACT,
        MODEL_PLAYLIST_INPUT_CONTRACT,
        MODEL_PLAYLIST_OUTPUT_CONTRACT,
        MODEL_PLAYLIST_DISCLOSURE_VERSION,
        MODEL_TAGGER_INPUT_CONTRACT,
        MODEL_TAGGER_OUTPUT_CONTRACT,
        MODEL_TAGGING_DISCLOSURE_VERSION,
        MODEL_TAG_CLEANUP_INPUT_CONTRACT,
        MODEL_TAG_CLEANUP_OUTPUT_CONTRACT,
        MODEL_TAG_CLEANUP_DISCLOSURE_VERSION,
        EQ_DRAFT_INPUT_CONTRACT,
        EQ_DRAFT_OUTPUT_CONTRACT,
        MODEL_EQ_DISCLOSURE_VERSION,
        MODEL_TAG_ANALYZER_ID,
        MODEL_TAG_CLEANUP_ENGINE_ID,
        EQ_DRAFT_ENGINE_ID,
        PLAYLIST_QUALITY_EVALUATION_ID,
        TAGGING_QUALITY_EVALUATION_ID,
        TAG_CLEANUP_QUALITY_EVALUATION_ID,
        EQ_QUALITY_EVALUATION_ID,
        *MODEL_ROLE_RUNTIME_CONTRACTS.values(),
    }
    missing = sorted(value for value in contract_values if value not in inventory)
    assert not missing, "Assistant contract inventory is missing: " + ", ".join(missing)

    missing_roles = sorted(
        role_id for role_id in MODEL_ROLE_RUNTIME_CONTRACTS if role_id not in inventory
    )
    assert not missing_roles, "Assistant contract inventory is missing roles: " + ", ".join(
        missing_roles
    )
