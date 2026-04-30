import argparse
import sys

from app.cli import create_user, init_storage, reload_modes, set_password


def main(argv: list[str] | None = None) -> int:
    parser = argparse.ArgumentParser(prog="music-cli")
    sub = parser.add_subparsers(dest="command", required=True)

    create_user.add_parser(sub)
    set_password.add_parser(sub)
    reload_modes.add_parser(sub)
    init_storage.add_parser(sub)

    args = parser.parse_args(argv)
    return args.handler(args)


if __name__ == "__main__":
    sys.exit(main())
