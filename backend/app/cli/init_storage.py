"""`music-cli init-storage` — create the storage directories the backend
needs without touching anything that already exists.

Useful for operators who want to materialise the directory layout under a
freshly bind-mounted volume *before* the FastAPI app boots — e.g. to chown
them or pre-populate them by other means. Running this inside the container
is equivalent to letting the lifespan create the same dirs on first boot.
"""
from __future__ import annotations

import argparse
import shutil
from pathlib import Path

from app.core.config import get_settings


def add_parser(sub: argparse._SubParsersAction) -> None:
    p = sub.add_parser(
        "init-storage",
        help=(
            "Create music/, sfx/, modes/ under the configured storage roots "
            "if they don't exist. Existing dirs are left alone."
        ),
    )
    p.add_argument(
        "--seed",
        action="store_true",
        help=(
            "Also copy bundled defaults into modes/ when it's empty (EQ "
            "presets ride along per-mode). Defaults to off when run from the "
            "CLI; the lifespan does this automatically."
        ),
    )
    p.set_defaults(handler=run)


def _seed_if_empty(target: Path, seed: Path | None, label: str) -> bool:
    """Copy `seed` into `target` if `target` is empty. Returns True if we
    actually copied something."""
    if seed is None or not seed.is_dir():
        return False
    if any(target.iterdir()):
        return False
    for entry in seed.iterdir():
        dest = target / entry.name
        if entry.is_dir():
            shutil.copytree(entry, dest)
        else:
            shutil.copy2(entry, dest)
    print(f"seeded {label} from {seed}")
    return True


def run(args: argparse.Namespace) -> int:
    settings = get_settings()
    targets: list[tuple[str, Path]] = [
        ("music", settings.music_dir.resolve()),
        ("sfx", settings.sfx_library_dir.resolve()),
        ("modes", settings.modes_dir.resolve()),
    ]
    for label, path in targets:
        existed = path.is_dir()
        path.mkdir(parents=True, exist_ok=True)
        print(f"{label:8s} {path} {'(existed)' if existed else '(created)'}")

    if args.seed:
        _seed_if_empty(
            settings.modes_dir.resolve(), settings.modes_seed_dir, "modes"
        )
    return 0
