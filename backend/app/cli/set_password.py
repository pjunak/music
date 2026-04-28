import argparse
import getpass

from sqlalchemy import select

from app.core.db import SessionLocal
from app.core.security import hash_password
from app.models.auth_session import AuthSession
from app.models.user import User


def add_parser(sub: argparse._SubParsersAction) -> None:
    p = sub.add_parser("set-password", help="Update an existing user's password.")
    p.add_argument("username")
    p.add_argument(
        "--password",
        help="New password. If omitted, prompts interactively (preferred — keeps the value out of shell history).",
    )
    p.add_argument(
        "--keep-sessions",
        action="store_true",
        help="By default we invalidate every existing session for this user (so a leaked cookie can't survive). Pass this flag to keep them.",
    )
    p.set_defaults(handler=run)


def run(args: argparse.Namespace) -> int:
    password = args.password or getpass.getpass("New password: ")
    if len(password) < 8:
        print("error: password must be at least 8 characters", flush=True)
        return 2

    with SessionLocal() as db:
        user = db.scalar(select(User).where(User.username == args.username))
        if user is None:
            print(f"error: user '{args.username}' not found", flush=True)
            return 1
        user.password_hash = hash_password(password)

        if not args.keep_sessions:
            killed = (
                db.query(AuthSession).filter(AuthSession.user_id == user.id).delete()
            )
        else:
            killed = 0

        db.commit()
        suffix = f" ({killed} active session(s) invalidated)" if killed else ""
        print(f"updated password for '{args.username}'{suffix}", flush=True)
    return 0
