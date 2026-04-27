import argparse

from app.modes import loader


def add_parser(sub: argparse._SubParsersAction) -> None:
    p = sub.add_parser("reload-modes", help="Load every mode manifest and print a summary.")
    p.set_defaults(handler=run)


def run(args: argparse.Namespace) -> int:
    result = loader.load_all()
    if not result.loaded and not result.errors:
        print("no modes loaded")
        return 1
    for mode in result.loaded.values():
        print(
            f"- {mode.id}: {mode.name} "
            f"({len(mode.panels)} panels, {len(mode.soundboards)} soundboards, "
            f"{len(mode.scenes)} scenes)"
        )
    for mode_id, error in result.errors.items():
        print(f"! {mode_id}: {error}")
    return 0 if not result.errors else 2
