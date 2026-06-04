"""Persistent device registry — read/write helpers over the `known_devices`
table. Thin synchronous functions taking a `Session`; the async sync layer
calls them through `run_in_threadpool`, mirroring `domain.playback_state`.

The cardinal rule: **`upsert_seen` must never touch `is_output`.** Output
eligibility is set only through the explicit operator path (`set_is_output`),
so a device reconnecting can't grant itself output authority.
"""
from __future__ import annotations

from sqlalchemy import select

from sqlalchemy.orm import Session

from app.models.base import utcnow
from app.models.known_device import KnownDevice


def upsert_seen(db: Session, client_id: str, name: str) -> KnownDevice:
    """Record that `client_id` just connected. Inserts a row (is_output=False)
    if new, otherwise refreshes the name + last_seen. Leaves `is_output` alone."""
    row = db.get(KnownDevice, client_id)
    if row is None:
        row = KnownDevice(client_id=client_id, name=name or "")
        db.add(row)
    else:
        if name:
            row.name = name
        row.last_seen = utcnow()
    db.commit()
    db.refresh(row)
    return row


def get(db: Session, client_id: str) -> KnownDevice | None:
    return db.get(KnownDevice, client_id)


def list_all(db: Session) -> list[KnownDevice]:
    """Every known device, outputs first then most-recently-seen — the order
    the operator's device list wants."""
    stmt = select(KnownDevice).order_by(
        KnownDevice.is_output.desc(), KnownDevice.last_seen.desc()
    )
    return list(db.scalars(stmt).all())


def set_is_output(db: Session, client_id: str, value: bool) -> KnownDevice | None:
    row = db.get(KnownDevice, client_id)
    if row is None:
        return None
    row.is_output = value
    db.commit()
    db.refresh(row)
    return row


def rename(db: Session, client_id: str, name: str) -> KnownDevice | None:
    row = db.get(KnownDevice, client_id)
    if row is None:
        return None
    row.name = name
    db.commit()
    db.refresh(row)
    return row


def delete(db: Session, client_id: str) -> bool:
    row = db.get(KnownDevice, client_id)
    if row is None:
        return False
    db.delete(row)
    db.commit()
    return True


def output_client_ids(db: Session) -> set[str]:
    """The client_ids designated as audio outputs — the authoritative answer
    to 'which devices may be activated as outputs'."""
    stmt = select(KnownDevice.client_id).where(KnownDevice.is_output.is_(True))
    return {row for row in db.scalars(stmt).all()}
