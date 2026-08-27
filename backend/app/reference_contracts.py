"""Deterministic contract snapshots for the Python-to-Rust rewrite.

This module is temporary migration infrastructure.  It turns the current
Python application's declared API, wire, persistence, and authored-file
contracts into reviewable files that both implementations can test against.
It deliberately reads model metadata only; runtime databases, credentials,
device registries, and media are never opened.
"""

from __future__ import annotations

import argparse
import hashlib
import json
from pathlib import Path
from typing import Annotated, Any

from pydantic import Field, TypeAdapter
from sqlalchemy.dialects import sqlite
from sqlalchemy.schema import CreateIndex, CreateTable

from app.main import app
from app.models import Base
from app.modes.loader import CueSpec, ModeManifest, SoundboardManifest
from app.presets.loader import PresetManifest
from app.sync.protocol import (
    ErrorMessage,
    SfxFired,
    StateChanged,
    StateSnapshot,
    action_adapter,
)

BASELINE_COMMIT = "b93f91d"
EXPORTER_VERSION = 1
REFERENCE_DIR = Path(__file__).resolve().parents[2] / "contracts" / "reference" / "v1"

_HTTP_METHODS = frozenset({"delete", "get", "head", "options", "patch", "post", "put", "trace"})

ServerMessage = Annotated[
    StateSnapshot | StateChanged | SfxFired | ErrorMessage,
    Field(discriminator="type"),
]
server_message_adapter: TypeAdapter[ServerMessage] = TypeAdapter(ServerMessage)


def _json_bytes(value: Any) -> bytes:
    return (json.dumps(value, indent=2, sort_keys=True, ensure_ascii=False) + "\n").encode()


def _sqlite_ddl() -> bytes:
    dialect = sqlite.dialect()
    statements: list[str] = []
    for table in Base.metadata.sorted_tables:
        table_ddl = str(CreateTable(table).compile(dialect=dialect)).strip()
        statements.append("\n".join(line.rstrip() for line in table_ddl.splitlines()))
        for index in sorted(table.indexes, key=lambda item: item.name or ""):
            statements.append(str(CreateIndex(index).compile(dialect=dialect)).strip())
    return (";\n\n".join(statements) + ";\n").encode()


def _authored_file_schemas() -> dict[str, Any]:
    return {
        "cue": CueSpec.model_json_schema(),
        "mode": ModeManifest.model_json_schema(),
        "preset": PresetManifest.model_json_schema(),
        "soundboard": SoundboardManifest.model_json_schema(),
    }


def build_reference_bundle() -> dict[str, bytes]:
    """Return every generated artifact, including its integrity manifest."""
    openapi = app.openapi()
    actions = action_adapter.json_schema()
    messages = server_message_adapter.json_schema()
    artifacts = {
        "authored-files.schema.json": _json_bytes(_authored_file_schemas()),
        "openapi.json": _json_bytes(openapi),
        "sqlite-schema.sql": _sqlite_ddl(),
        "websocket-actions.schema.json": _json_bytes(actions),
        "websocket-messages.schema.json": _json_bytes(messages),
    }

    operations = sum(
        method in _HTTP_METHODS
        for path_item in openapi.get("paths", {}).values()
        for method in path_item
    )
    manifest = {
        "artifacts": {
            name: {
                "bytes": len(content),
                "sha256": hashlib.sha256(content).hexdigest(),
            }
            for name, content in sorted(artifacts.items())
        },
        "baseline_commit": BASELINE_COMMIT,
        "counts": {
            "http_operations": operations,
            "http_paths": len(openapi.get("paths", {})),
            "openapi_schemas": len(openapi.get("components", {}).get("schemas", {})),
            "sqlite_tables": len(Base.metadata.sorted_tables),
            "websocket_actions": len(actions.get("oneOf", [])),
            "websocket_messages": len(messages.get("oneOf", [])),
        },
        "exporter_version": EXPORTER_VERSION,
    }
    artifacts["manifest.json"] = _json_bytes(manifest)
    return artifacts


def write_reference_bundle(target: Path = REFERENCE_DIR) -> None:
    target.mkdir(parents=True, exist_ok=True)
    for name, content in build_reference_bundle().items():
        (target / name).write_bytes(content)


def reference_drift(target: Path = REFERENCE_DIR) -> list[str]:
    expected = build_reference_bundle()
    actual_names = {path.name for path in target.iterdir() if path.is_file()} if target.is_dir() else set()
    drift = [name for name, content in expected.items() if not (target / name).is_file() or (target / name).read_bytes() != content]
    drift.extend(sorted(actual_names - expected.keys()))
    return sorted(set(drift))


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    mode = parser.add_mutually_exclusive_group(required=True)
    mode.add_argument("--check", action="store_true", help="fail when checked-in fixtures drift")
    mode.add_argument("--write", action="store_true", help="regenerate checked-in fixtures")
    parser.add_argument("--target", type=Path, default=REFERENCE_DIR)
    args = parser.parse_args()

    if args.write:
        write_reference_bundle(args.target)
        return 0

    drift = reference_drift(args.target)
    if drift:
        parser.error(
            "reference contract drift: "
            + ", ".join(drift)
            + "; regenerate with python -m app.reference_contracts --write"
        )
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
