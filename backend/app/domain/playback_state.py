"""Singleton playback state held in a single DB row (id=1).

Holds active mode, queue, output device, etc. — anything that should
survive restart. Wraps the row so callers don't have to think about
JSON-column mutation tracking (assignment of a new dict is required for
SQLAlchemy to detect a change).
"""
from __future__ import annotations

from typing import Any

from sqlalchemy.orm import Session

from app.models.playback_state import PlaybackState


def _ensure_row(db: Session) -> PlaybackState:
    row = db.get(PlaybackState, 1)
    if row is None:
        row = PlaybackState(id=1, state_json={})
        db.add(row)
        db.commit()
        db.refresh(row)
    return row


def get_state(db: Session) -> dict[str, Any]:
    row = _ensure_row(db)
    return dict(row.state_json or {})


def update_state(db: Session, **updates: Any) -> dict[str, Any]:
    """Merge ``updates`` into the singleton state. Returns the new state."""
    row = _ensure_row(db)
    new_state = dict(row.state_json or {})
    new_state.update(updates)
    row.state_json = new_state  # reassignment so SQLAlchemy sees the change
    db.commit()
    return new_state
