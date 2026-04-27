import argparse
import getpass

from sqlalchemy import select

from app.core.db import SessionLocal
from app.core.security import hash_password
from app.models.user import User


def add_parser(sub: argparse._SubParsersAction) -> None:
    p = sub.add_parser("create-user", help="Create the admin user.")
    p.add_argument("username")
    p.add_argument("--password", help="Password. If omitted, prompts interactively.")
    p.set_defaults(handler=run)


def run(args: argparse.Namespace) -> int:
    password = args.password or getpass.getpass("Password: ")
    if len(password) < 8:
        print("error: password must be at least 8 characters", flush=True)
        return 2

    with SessionLocal() as db:
        existing = db.scalar(select(User).where(User.username == args.username))
        if existing is not None:
            print(f"error: user '{args.username}' already exists", flush=True)
            return 1
        user = User(username=args.username, password_hash=hash_password(password))
        db.add(user)
        db.commit()
        print(f"created user '{args.username}' (id={user.id})", flush=True)
    return 0
