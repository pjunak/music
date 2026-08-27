"""Deterministic contract snapshots for the Python-to-Rust rewrite.

This module is temporary migration infrastructure.  It turns the current
Python application's declared API, wire, persistence, and authored-file
contracts into reviewable files that both implementations can test against.
It deliberately reads model metadata only; runtime databases, credentials,
device registries, and media are never opened.
"""

from __future__ import annotations

import argparse
import base64
import hashlib
import json
import sqlite3
import tempfile
from pathlib import Path
from typing import Annotated, Any

import yaml
from argon2.low_level import Type, hash_secret
from cryptography.hazmat.primitives.ciphers.aead import AESGCM
from mutagen import File as MutagenFile  # type: ignore[import-untyped]
from mutagen.flac import Picture  # type: ignore[import-untyped]
from mutagen.id3 import APIC, TXXX  # type: ignore[import-untyped]
from mutagen.mp4 import MP4Cover, MP4FreeForm  # type: ignore[import-untyped]
from pydantic import Field, TypeAdapter
from sqlalchemy.dialects import sqlite
from sqlalchemy.schema import CreateIndex, CreateTable

from app.library import index as library_index
from app.main import app
from app.models import Base
from app.modes.loader import CueSpec, ModeManifest, SoundboardManifest
from app.presets.loader import PresetManifest
from app.reference_cases import (
    INVALID_WEBSOCKET_ACTIONS,
    INVALID_WEBSOCKET_MESSAGES,
    VALID_WEBSOCKET_ACTIONS,
    VALID_WEBSOCKET_MESSAGES,
)
from app.reference_media import (
    FFMPEG_FIXTURE_PROVENANCE,
    REFERENCE_AUDIO_BUILDERS,
)
from app.sync.protocol import (
    ErrorMessage,
    SfxFired,
    StateChanged,
    StateSnapshot,
    action_adapter,
)

BASELINE_COMMIT = "b93f91d"
EXPORTER_VERSION = 2
REFERENCE_DIR = Path(__file__).resolve().parents[2] / "contracts" / "reference" / "v1"
PROJECT_ROOT = Path(__file__).resolve().parents[2]

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


def _compatibility_data() -> dict[str, Any]:
    password = "rewrite-fixture-password"
    password_hash = hash_secret(
        password.encode(),
        b"rewrite-fixture-salt",
        time_cost=3,
        memory_cost=65536,
        parallelism=4,
        hash_len=32,
        type=Type.ID,
        version=19,
    ).decode("ascii")

    connection_id = "0123456789abcdef0123456789abcdef"
    credential_key = bytes(range(32))
    credential_nonce = bytes(range(12))
    credential_plaintext = "fixture-api-key-not-a-secret"
    credential_aad = f"assistant-provider-credential/v1:{connection_id}"
    credential_ciphertext = AESGCM(credential_key).encrypt(
        credential_nonce,
        credential_plaintext.encode(),
        credential_aad.encode("ascii"),
    )

    legacy_devices = {
        "living-room": {
            "added_at": "2026-08-27T10:00:00+00:00",
            "is_output": True,
            "name": "Living Room",
        },
        "tablet": {
            "added_at": "2026-08-27T10:05:00+00:00",
            "is_output": False,
            "name": "Tabletop Controller",
        },
        "ignored-non-record": ["not", "an", "object"],
    }
    return {
        "aes_256_gcm": {
            "aad": credential_aad,
            "ciphertext_urlsafe_base64": base64.urlsafe_b64encode(
                credential_ciphertext
            ).decode("ascii"),
            "connection_id": connection_id,
            "key_id": hashlib.sha256(credential_key).hexdigest()[:16],
            "key_urlsafe_base64": base64.urlsafe_b64encode(credential_key).decode(
                "ascii"
            ),
            "nonce_urlsafe_base64": base64.urlsafe_b64encode(
                credential_nonce
            ).decode("ascii"),
            "plaintext": credential_plaintext,
        },
        "argon2id": {
            "invalid_password": "rewrite-fixture-password-wrong",
            "password": password,
            "phc": password_hash,
        },
        "legacy_device_cases": [
            {"expected": [], "id": "missing-file", "source": None},
            {"expected": [], "id": "corrupt-json", "source": "{not-json"},
            {"expected": [], "id": "non-object-root", "source": "[]\n"},
            {
                "expected": [
                    {
                        "added_at": "2026-08-27T10:00:00+00:00",
                        "client_id": "living-room",
                        "is_output": True,
                        "name": "Living Room",
                    },
                    {
                        "added_at": "2026-08-27T10:05:00+00:00",
                        "client_id": "tablet",
                        "is_output": False,
                        "name": "Tabletop Controller",
                    },
                ],
                "id": "representative",
                "source": json.dumps(
                    legacy_devices, indent=2, sort_keys=True, ensure_ascii=False
                )
                + "\n",
            },
        ],
        "sqlite": {
            "representative_rows_per_table": 1,
            "table_count": len(Base.metadata.sorted_tables),
            "timestamp": "2026-08-27 12:34:56.000000",
        },
        "version": 1,
    }


def _sqlite_fixture_sql(compatibility: dict[str, Any]) -> bytes:
    timestamp = compatibility["sqlite"]["timestamp"]
    credential = compatibility["aes_256_gcm"]
    password = compatibility["argon2id"]

    def compact(value: Any) -> str:
        return json.dumps(
            value, sort_keys=True, separators=(",", ":"), ensure_ascii=False
        )

    database = sqlite3.connect(":memory:")
    try:
        database.execute("PRAGMA foreign_keys=ON")
        database.executescript(_sqlite_ddl().decode("utf-8"))
        database.execute(
            "INSERT INTO assistant_provider_connections VALUES "
            "(?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)",
            (
                credential["connection_id"],
                "Fixture provider",
                "openai-responses",
                "https://example.invalid/v1",
                credential["ciphertext_urlsafe_base64"],
                credential["nonce_urlsafe_base64"],
                "••••cret",
                0,
                "verified",
                None,
                compact(["fixture-model"]),
                compact(["structured-output"]),
                timestamp,
                timestamp,
                timestamp,
            ),
        )
        database.execute(
            "INSERT INTO assistant_tag_vocabularies VALUES (?, ?, ?, ?, ?)",
            ("default", 1, 1, compact({"groups": []}), timestamp),
        )
        database.execute(
            "INSERT INTO background_jobs VALUES "
            "(?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)",
            (
                "fixture-job",
                "library_context",
                "succeeded",
                compact({"track_ids": [1]}),
                compact({"processed": 1}),
                None,
                1,
                1,
                "complete",
                "Fixture complete",
                1,
                None,
                timestamp,
                timestamp,
                timestamp,
                timestamp,
            ),
        )
        database.execute(
            "INSERT INTO cleanup_batches VALUES (?, ?, ?, ?, ?)",
            (1, timestamp, "Fixture scope", compact([{"track_id": 1}]), None),
        )
        database.execute(
            "INSERT INTO cleanup_name_lookups VALUES (?, ?, ?, ?, ?, ?)",
            (1, "fixture title", "Fixture Title", 90, 80, timestamp),
        )
        database.execute(
            "INSERT INTO playback_state VALUES (?, ?, ?)",
            (
                1,
                compact(
                    {
                        "active_output_device_ids": ["living-room"],
                        "is_playing": False,
                        "revision": 7,
                    }
                ),
                timestamp,
            ),
        )
        database.execute(
            "INSERT INTO playlists VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?)",
            (
                1,
                "Fixture playlist",
                "dnd",
                "ambient",
                compact({}),
                None,
                None,
                timestamp,
                timestamp,
            ),
        )
        database.execute(
            "INSERT INTO tracks VALUES "
            "(?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)",
            (
                1,
                "Fixture/track.wav",
                "Fixture Track",
                "Fixture Artist",
                "Fixture Album Artist",
                "Fixture Album",
                1,
                1,
                2026,
                "Soundtrack",
                12.5,
                120,
                "Fixture Artist — Fixture Track",
                "fixture",
                24044,
                1787826896,
                timestamp,
            ),
        )
        database.execute(
            "INSERT INTO users VALUES (?, ?, ?, ?)",
            (1, "fixture-user", password["phc"], timestamp),
        )
        database.execute(
            "INSERT INTO assistant_model_roles VALUES "
            "(?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)",
            (
                "playlist",
                credential["connection_id"],
                "fixture-model",
                1,
                30,
                2048,
                "provider_default",
                "passed",
                None,
                "fixture-fingerprint",
                timestamp,
                timestamp,
            ),
        )
        database.execute(
            "INSERT INTO auth_sessions VALUES (?, ?, ?, ?, ?)",
            ("fixture-session-token", 1, timestamp, timestamp, timestamp),
        )
        database.execute(
            "INSERT INTO playlist_items VALUES (?, ?, ?, ?)",
            (1, 0, 1, timestamp),
        )
        database.execute(
            "INSERT INTO track_analyses VALUES "
            "(?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)",
            (
                1,
                "fixture-signal-v1",
                "fixture-source",
                "fixture-job",
                0.4,
                0.3,
                0.2,
                compact(["mood.calm"]),
                compact({"source": "fixture"}),
                compact({"rms": 0.1}),
                "medium",
                timestamp,
            ),
        )
        database.execute(
            "INSERT INTO track_analysis_failures VALUES (?, ?, ?, ?, ?, ?)",
            (
                1,
                "fixture-failed-v1",
                "fixture-source",
                "fixture-job",
                "synthetic failure",
                timestamp,
            ),
        )
        database.execute(
            "INSERT INTO track_analysis_tag_reviews VALUES (?, ?, ?, ?, ?, ?)",
            (
                1,
                "fixture-signal-v1",
                "mood.calm",
                "fixture-source",
                "accepted",
                timestamp,
            ),
        )
        database.execute(
            "INSERT INTO track_contexts VALUES "
            "(?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)",
            (
                1,
                "fixture-context-v1",
                "fixture-source",
                "fixture-job",
                "complete",
                "medium",
                compact({"label": "Fixture"}),
                compact([]),
                compact([]),
                compact({"duration_s": 12.5}),
                compact({"signal": "complete"}),
                timestamp,
            ),
        )
        database.execute(
            "INSERT INTO track_user_tags VALUES (?, ?, ?)",
            (1, "mood.calm", timestamp),
        )
        database.execute(
            "INSERT INTO assistant_model_evaluations VALUES "
            "(?, ?, ?, ?, ?, ?, ?, ?, ?, ?)",
            (
                "playlist",
                "fixture-evaluation",
                "fixture-fingerprint",
                "passed",
                "playlist-model-v1",
                "fixture-engine",
                1,
                1,
                "fixture-job",
                timestamp,
            ),
        )
        database.commit()
        dump = "\n".join(database.iterdump())
        return (
            "PRAGMA foreign_keys=OFF;\n"
            + dump
            + "\nPRAGMA foreign_keys=ON;\n"
        ).encode()
    finally:
        database.close()


def _authored_file_schemas() -> dict[str, Any]:
    return {
        "cue": CueSpec.model_json_schema(),
        "mode": ModeManifest.model_json_schema(),
        "preset": PresetManifest.model_json_schema(),
        "soundboard": SoundboardManifest.model_json_schema(),
    }


def _authored_file_examples() -> dict[str, Any]:
    def build_case(path: Path, source: str) -> dict[str, Any]:
        parsed = yaml.safe_load(source)
        if not isinstance(parsed, dict):
            raise ValueError(f"authored YAML must contain a mapping: {path}")
        data = dict(parsed)

        if path.name == "manifest.yaml":
            kind = "mode"
            model = ModeManifest.model_validate(data)
            canonical = model.model_dump(
                mode="json",
                exclude={"root_dir", "soundboards", "cues", "presets"},
            )
        elif path.parent.name == "soundboards":
            kind = "soundboard"
            data.setdefault("id", path.stem)
            canonical = SoundboardManifest.model_validate(data).model_dump(mode="json")
        elif path.parent.name == "cues":
            kind = "cue"
            data.setdefault("id", path.stem)
            canonical = CueSpec.model_validate(data).model_dump(mode="json")
        elif path.parent.name == "presets":
            kind = "preset"
            data.setdefault("id", path.stem)
            preset = PresetManifest.model_validate(data)
            for effect in preset.effects:
                effect.validate_type()
            canonical = preset.model_dump(mode="json")
        else:
            raise ValueError(f"unrecognized authored YAML location: {path}")

        return {
            "canonical": canonical,
            "kind": kind,
            "path": path.relative_to(PROJECT_ROOT).as_posix(),
            "source": source,
            "source_sha256": hashlib.sha256(source.encode()).hexdigest(),
        }

    modes_root = PROJECT_ROOT / "modes"
    cases = [
        build_case(path, path.read_text(encoding="utf-8"))
        for path in sorted(modes_root.rglob("*.yaml"))
    ]
    synthetic_root = PROJECT_ROOT / "contracts" / "reference" / "synthetic"
    synthetic_files = [
        (
            synthetic_root / "soundboards" / "tavern.yaml",
            "name: Tavern\n"
            "categories:\n"
            "  - id: doors\n"
            "    name: Doors\n"
            "    items:\n"
            "      - file: dnd/door.ogg\n"
            "        name: Door slam\n"
            "        hotkey: d\n",
        ),
        (
            synthetic_root / "cues" / "kraken.yaml",
            "name: Release the Kraken\n"
            "description: Full authored cue fixture\n"
            "preset: cave\n"
            "playlist: Combat\n"
            "start_index: 1\n"
            "start_ms: 500\n"
            "sfx:\n"
            "  - soundboard: tavern\n"
            "    item: doors/0\n"
            "    volume: 0.75\n"
            "loops:\n"
            "  - soundboard: weather\n"
            "    item: rain/0\n"
            "    interval_s: 12.5\n",
        ),
        (
            synthetic_root / "presets" / "cave.yaml",
            "name: Cave\n"
            "description: Full authored preset fixture\n"
            "crossfade_ms: 1500\n"
            "effects:\n"
            "  - type: reverb\n"
            "    wet: 0.4\n"
            "  - type: highpass\n"
            "    frequency: 400\n",
        ),
    ]
    cases.extend(build_case(path, source) for path, source in synthetic_files)
    return {"cases": cases, "version": 1}


def _metadata_examples() -> dict[str, Any]:
    sample: dict[str, str | int] = {
        "title": "Round Trip",
        "artist": "The Artist",
        "album_artist": "Album Artist",
        "album": "An Album",
        "track_no": 7,
        "disc_no": 2,
        "year": 1991,
        "genre": "Ambient",
        "bpm": 123,
    }
    sentinel_key = "MUSIC_REWRITE_SENTINEL"
    sentinel_value = "keep-me"
    # A valid one-pixel PNG keeps picture-preservation checks synthetic and tiny.
    cover = base64.b64decode(
        "iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAQAAAC1HAwCAAAAC0lEQVR42mNk+A8AAQUB"
        "AScY42YAAAAASUVORK5CYII="
    )

    def add_preservation_fields(path: Path, extension: str) -> bool:
        media = MutagenFile(str(path))
        if media is None or media.tags is None:
            raise ValueError(f"synthetic {extension} fixture has no writable tags")
        if extension in {"aiff", "mp3", "wav"}:
            media.tags.add(TXXX(encoding=3, desc=sentinel_key, text=[sentinel_value]))
            media.tags.add(
                APIC(
                    encoding=3,
                    mime="image/png",
                    type=3,
                    desc="rewrite-fixture",
                    data=cover,
                )
            )
            artwork_expected = True
        elif extension == "m4a":
            media.tags[f"----:com.music-streaming:{sentinel_key}"] = [
                MP4FreeForm(sentinel_value.encode("utf-8"))
            ]
            media.tags["covr"] = [
                MP4Cover(cover, imageformat=MP4Cover.FORMAT_PNG)
            ]
            artwork_expected = True
        else:
            media.tags[sentinel_key] = [sentinel_value]
            artwork_expected = extension in {"flac", "opus"}
            if artwork_expected:
                picture = Picture()
                picture.type = 3
                picture.mime = "image/png"
                picture.desc = "rewrite-fixture"
                picture.data = cover
                if extension == "flac":
                    media.add_picture(picture)
                else:
                    media.tags["metadata_block_picture"] = [
                        base64.b64encode(picture.write()).decode("ascii")
                    ]
        media.save()
        return artwork_expected

    cases: list[dict[str, Any]] = []
    with tempfile.TemporaryDirectory(prefix="music-reference-media-") as temp_dir:
        root = Path(temp_dir)
        for extension, builder in sorted(REFERENCE_AUDIO_BUILDERS.items()):
            path = root / f"track.{extension}"
            path.write_bytes(builder())
            write_supported = extension != "aac"
            legacy_write_error: str | None = None
            artwork_expected = False
            if write_supported:
                library_index.write_tags(path, sample)
                artwork_expected = add_preservation_fields(path, extension)
            else:
                source_before_attempt = path.read_bytes()
                try:
                    library_index.write_tags(path, sample)
                except Exception as error:
                    legacy_write_error = (
                        f"{type(error).__module__}.{type(error).__qualname__}"
                    )
                else:
                    raise ValueError("raw AAC unexpectedly accepted metadata writes")
                if path.read_bytes() != source_before_attempt:
                    raise ValueError("failed raw AAC metadata write changed the source")
            source = path.read_bytes()
            raw_metadata = library_index._read_tags(path)
            canonical = {
                key: raw_metadata.get(key, None if isinstance(value, int) else "")
                for key, value in sample.items()
            }
            if write_supported:
                for key, expected in sample.items():
                    if canonical[key] != expected:
                        raise ValueError(f"{extension} fixture lost {key}")
            cases.append(
                {
                    "artwork_expected": artwork_expected,
                    "canonical": {key: canonical[key] for key in sample},
                    "duration_millis": round(
                        float(raw_metadata.get("length_s", 0.0)) * 1000
                    ),
                    "extension": f".{extension}",
                    "legacy_write_error": legacy_write_error,
                    "metadata_write_supported": write_supported,
                    "preservation_markers": (
                        [sentinel_key, sentinel_value] if write_supported else []
                    ),
                    "source_base64": base64.b64encode(source).decode("ascii"),
                    "source_sha256": hashlib.sha256(source).hexdigest(),
                }
            )

    covered = {case["extension"] for case in cases}
    runtime_extensions = sorted(library_index.AUDIO_EXTENSIONS)
    return {
        "cases": cases,
        "covered_extensions": sorted(covered),
        "ffmpeg_fixture_provenance": FFMPEG_FIXTURE_PROVENANCE,
        "read_only_extensions": [".aac"],
        "runtime_extensions": runtime_extensions,
        "version": 1,
        "writable_fields": list(library_index.WRITABLE_TAGS),
        "write_supported_extensions": sorted(covered - {".aac"}),
    }


def _websocket_action_examples() -> dict[str, Any]:
    valid: list[dict[str, Any]] = []
    action_types: set[str] = set()
    for case in VALID_WEBSOCKET_ACTIONS:
        action = action_adapter.validate_python(case["input"])
        canonical = action.model_dump(mode="json")
        action_types.add(canonical["type"])
        valid.append({"canonical": canonical, **case})

    schema = action_adapter.json_schema()
    schema_types = {
        reference.rsplit("/", maxsplit=1)[-1]
        for reference in schema["discriminator"]["mapping"].values()
    }
    model_types = {type(action_adapter.validate_python(case["input"])).__name__ for case in valid}
    if model_types != schema_types:
        missing = sorted(schema_types - model_types)
        extra = sorted(model_types - schema_types)
        raise ValueError(f"WebSocket action corpus mismatch: missing={missing}, extra={extra}")

    for case in INVALID_WEBSOCKET_ACTIONS:
        try:
            action_adapter.validate_python(case["input"])
        except ValueError:
            continue
        raise ValueError(f"invalid WebSocket action was accepted: {case['id']}")

    return {
        "action_types": sorted(action_types),
        "invalid": INVALID_WEBSOCKET_ACTIONS,
        "valid": valid,
        "version": 1,
    }


def _websocket_message_examples() -> dict[str, Any]:
    valid: list[dict[str, Any]] = []
    message_types: set[str] = set()
    for case in VALID_WEBSOCKET_MESSAGES:
        message = server_message_adapter.validate_python(case["input"])
        canonical = message.model_dump(mode="json")
        message_types.add(canonical["type"])
        valid.append({"canonical": canonical, **case})

    for case in INVALID_WEBSOCKET_MESSAGES:
        try:
            server_message_adapter.validate_python(case["input"])
        except ValueError:
            continue
        raise ValueError(f"invalid WebSocket message was accepted: {case['id']}")

    return {
        "invalid": INVALID_WEBSOCKET_MESSAGES,
        "message_types": sorted(message_types),
        "valid": valid,
        "version": 1,
    }


def build_reference_bundle() -> dict[str, bytes]:
    """Return every generated artifact, including its integrity manifest."""
    openapi = app.openapi()
    actions = action_adapter.json_schema()
    messages = server_message_adapter.json_schema()
    compatibility = _compatibility_data()
    authored_examples = _authored_file_examples()
    metadata_examples = _metadata_examples()
    artifacts = {
        "authored-files.examples.json": _json_bytes(authored_examples),
        "authored-files.schema.json": _json_bytes(_authored_file_schemas()),
        "compatibility-data.json": _json_bytes(compatibility),
        "metadata.examples.json": _json_bytes(metadata_examples),
        "openapi.json": _json_bytes(openapi),
        "sqlite-fixture.sql": _sqlite_fixture_sql(compatibility),
        "sqlite-schema.sql": _sqlite_ddl(),
        "websocket-actions.examples.json": _json_bytes(_websocket_action_examples()),
        "websocket-actions.schema.json": _json_bytes(actions),
        "websocket-messages.examples.json": _json_bytes(_websocket_message_examples()),
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
            "authored_file_examples": len(authored_examples["cases"]),
            "legacy_device_cases": len(compatibility["legacy_device_cases"]),
            "metadata_examples": len(metadata_examples["cases"]),
            "openapi_schemas": len(openapi.get("components", {}).get("schemas", {})),
            "sqlite_fixture_rows": (
                compatibility["sqlite"]["table_count"]
                * compatibility["sqlite"]["representative_rows_per_table"]
            ),
            "sqlite_tables": len(Base.metadata.sorted_tables),
            "websocket_actions": len(actions.get("oneOf", [])),
            "websocket_action_examples": len(VALID_WEBSOCKET_ACTIONS),
            "websocket_action_rejections": len(INVALID_WEBSOCKET_ACTIONS),
            "websocket_messages": len(messages.get("oneOf", [])),
            "websocket_message_examples": len(VALID_WEBSOCKET_MESSAGES),
            "websocket_message_rejections": len(INVALID_WEBSOCKET_MESSAGES),
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
