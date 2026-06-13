"""Library-cleanup HTTP surface.

`analyze` is pure (DB reads + string heuristics — see
`app/library/cleanup.py`); the client shows the result as a reviewable
diff. `apply` performs only the operations the operator accepted, with a
per-op stale-check (old value must still match) so a plan from a stale
analysis degrades to per-op skips instead of clobbering newer edits.
Everything that lands is journaled to a `cleanup_batches` row that can be
listed, downloaded as JSON, and reverted — including from an uploaded
journal file after a DB wipe (items carry paths, not just track ids).
"""
from __future__ import annotations

import contextlib
import json
import logging
import shutil
from datetime import datetime
from pathlib import Path, PurePosixPath
from typing import Any, Literal

from fastapi import APIRouter, HTTPException, status
from pydantic import BaseModel, Field
from sqlalchemy import select

from app.api.deps import CurrentUser, DbSession
from app.library import cleanup as engine
from app.library import cleanup_lookup
from app.library import index as library_index
from app.models.base import utcnow
from app.models.cleanup_batch import CleanupBatch
from app.models.cleanup_lookup import CleanupNameLookup
from app.models.track import Track

logger = logging.getLogger(__name__)
router = APIRouter(prefix="/api/library/cleanup", tags=["library-cleanup"])

RuleId = Literal[
    "strip_track_numbers",
    "strip_artist",
    "strip_album",
    "strip_junk",
    "normalize_separators",
    "normalize_case",
    "tag_title",
    "tag_artist",
    "tag_album",
    "tag_number",
    "tag_year",
]

# Tag fields apply will write. The engine only suggests a subset, but the
# apply path validates against this set so a hand-crafted op can't write
# arbitrary attributes.
_TAG_FIELDS: frozenset[str] = frozenset(
    {"title", "artist", "album_artist", "album", "track_no", "disc_no", "year"}
)


# --- schemas ---------------------------------------------------------------


class CleanupScope(BaseModel):
    type: Literal["all", "folder", "tracks"]
    path: str = ""
    recursive: bool = True
    track_ids: list[int] = Field(default_factory=list, max_length=5000)


class AnalyzeRequest(BaseModel):
    scope: CleanupScope
    rules: list[RuleId] | None = Field(
        None, description="Enabled rule ids; omit for the default set"
    )


class CleanupOpOut(BaseModel):
    op_id: str
    track_id: int
    kind: Literal["rename", "tag"]
    field: str | None
    old: int | str | None
    new: int | str | None
    rules: list[str]
    confidence: Literal["high", "low"]
    verified: bool = False


class CleanupTrackPlanOut(BaseModel):
    track_id: int
    path: str
    ops: list[CleanupOpOut]
    notes: list[str]


class AnalyzeResult(BaseModel):
    scanned: int
    plans: list[CleanupTrackPlanOut]
    # Names an online lookup could still settle (deduped; not yet cached).
    # The client resolves them via POST /verify and re-analyzes — the
    # second pass picks the verdicts up from the cache.
    pending_lookups: list[str]


class CleanupOpIn(BaseModel):
    track_id: int
    kind: Literal["rename", "tag"]
    field: str | None = None
    old: int | str | None = None
    new: int | str | None = None


class ApplyRequest(BaseModel):
    """One chunk of accepted operations. The client slices a big apply into
    chunks for a live progress bar; the first response mints the batch and
    later chunks pass `batch_id` back so the whole run shares one journal."""

    ops: list[CleanupOpIn] = Field(min_length=1, max_length=500)
    batch_id: int | None = None
    scope_label: str = Field("", max_length=512)


class CleanupSkip(BaseModel):
    track_id: int
    reason: str


class ApplyResult(BaseModel):
    # None when nothing in the run has applied yet (no journal to keep).
    batch_id: int | None
    applied: int
    skipped: list[CleanupSkip]


class BatchSummary(BaseModel):
    id: int
    created_at: datetime
    scope_label: str
    item_count: int
    reverted_at: datetime | None


class BatchDetail(BatchSummary):
    items: list[dict[str, Any]]


class RevertJournalRequest(BaseModel):
    items: list[dict[str, Any]] = Field(min_length=1, max_length=5000)


class RevertResult(BaseModel):
    reverted: int
    skipped: list[CleanupSkip]


# --- analyze ----------------------------------------------------------------


def _resolve_scope(db: DbSession, scope: CleanupScope) -> list[Track]:
    if scope.type == "tracks":
        if not scope.track_ids:
            raise HTTPException(
                status_code=status.HTTP_400_BAD_REQUEST,
                detail="scope.track_ids must be non-empty for a tracks scope",
            )
        return list(db.scalars(select(Track).where(Track.id.in_(scope.track_ids))).all())
    if scope.type == "folder":
        rel = scope.path.strip("/").replace("\\", "/")
        if not rel:
            if scope.recursive:
                return list(db.scalars(select(Track)).all())
            return list(db.scalars(select(Track).where(~Track.path.like("%/%"))).all())
        prefix = f"{rel}/"
        stmt = select(Track).where(Track.path.like(f"{prefix}%"))
        if not scope.recursive:
            stmt = stmt.where(~Track.path.like(f"{prefix}%/%"))
        return list(db.scalars(stmt).all())
    return list(db.scalars(select(Track)).all())


def _load_verdicts(db: DbSession) -> dict[str, tuple[int, int]]:
    """The whole lookup cache as the engine's verdict map. Names are only
    ever queried once — after that they ride along with every analysis for
    free, which is what lets the lookup act as just another clue instead
    of a separate verification step."""
    rows = db.scalars(select(CleanupNameLookup)).all()
    return {r.loose_key: (r.artist_score, r.album_score) for r in rows}


@router.post("/analyze", response_model=AnalyzeResult)
def analyze(payload: AnalyzeRequest, _: CurrentUser, db: DbSession) -> AnalyzeResult:
    scope_tracks = _resolve_scope(db, payload.scope)
    all_tracks = list(db.scalars(select(Track)).all())
    enabled = set(payload.rules) if payload.rules is not None else engine.DEFAULT_RULES
    verdicts = _load_verdicts(db)
    plans = engine.analyze(scope_tracks, all_tracks, enabled, verdicts)
    return AnalyzeResult(
        scanned=len(scope_tracks),
        # A plan may exist only to surface names worth looking up
        # (`wants_lookup`); those aren't visible changes — don't render
        # empty cards for them.
        plans=[
            CleanupTrackPlanOut(
                track_id=p.track_id,
                path=p.path,
                ops=[
                    CleanupOpOut(
                        op_id=o.op_id,
                        track_id=o.track_id,
                        kind=o.kind,  # type: ignore[arg-type]
                        field=o.field,
                        old=o.old,
                        new=o.new,
                        rules=list(o.rules),
                        confidence=o.confidence,  # type: ignore[arg-type]
                        verified=o.verified,
                    )
                    for o in p.ops
                ],
                notes=p.notes,
            )
            for p in plans
            if p.ops or p.notes
        ],
        pending_lookups=engine.pending_lookups(plans, verdicts),
    )


class VerifyRequest(BaseModel):
    """One small batch of names to resolve against MusicBrainz. Kept short
    (the lookup client paces at 1 request/second, two queries per name) so
    a synchronous call stays well under proxy timeouts; the client chunks
    a longer list and shows progress between calls."""

    names: list[str] = Field(min_length=1, max_length=5)


class VerifyResult(BaseModel):
    verified: int
    failed: list[str]


@router.post("/verify", response_model=VerifyResult)
def verify_names(payload: VerifyRequest, _: CurrentUser, db: DbSession) -> VerifyResult:
    """Resolve names and cache the verdicts. Idempotent: already-cached
    names are skipped, and failures aren't cached so they retry naturally
    on a later run. The next /analyze picks the fresh verdicts up."""
    verified = 0
    failed: list[str] = []
    for raw in payload.names:
        name = raw.strip()
        key = engine.loose_key(name)
        if len(key) < 2:
            continue
        existing = db.scalar(
            select(CleanupNameLookup).where(CleanupNameLookup.loose_key == key)
        )
        if existing is not None:
            continue
        try:
            artist_score, album_score = cleanup_lookup.fetch_name_scores(name)
        except Exception as e:
            logger.warning("name lookup failed for %r: %s", name, e)
            failed.append(name)
            continue
        db.add(
            CleanupNameLookup(
                loose_key=key,
                name=name[:512],
                artist_score=artist_score,
                album_score=album_score,
            )
        )
        # Commit per name: partial progress survives a failure later in
        # the batch (those names just retry next run).
        db.commit()
        verified += 1
    return VerifyResult(verified=verified, failed=failed)


# --- apply ------------------------------------------------------------------


def _rename_in_place(db: DbSession, track: Track, new_stem: str) -> tuple[str, str] | str:
    """Rename the track's file within its folder, keeping the index in sync.
    Returns (path_before, path_after) on success, a skip reason on failure."""
    ppath = PurePosixPath(track.path)
    src = library_index.to_absolute(track.path)
    if not src.is_file():
        return "source file missing on disk"
    if (
        not new_stem
        or new_stem != new_stem.strip()
        or any(c in new_stem for c in "/\\")
        or new_stem.startswith(".")
        or len(new_stem) > 255
    ):
        return f"invalid target name: {new_stem!r}"

    parent = str(ppath.parent) if str(ppath.parent) != "." else ""
    try:
        target = library_index.safe_join(parent, f"{new_stem}{ppath.suffix}")
    except ValueError as e:
        return f"invalid target name: {e}"
    if target == src:
        return "no change"
    # On a case-insensitive filesystem the target "exists" when the rename
    # only changes letter case — that one is legal, os.rename handles it.
    case_only = target.parent == src.parent and target.name.casefold() == src.name.casefold()
    if target.exists() and not case_only:
        return f"a file named {target.name} already exists"

    path_before = track.path
    shutil.move(str(src), str(target))
    path_after = library_index.to_relative(target)
    try:
        library_index.update_path(db, path_before, path_after)
    except Exception as e:
        with contextlib.suppress(Exception):
            shutil.move(str(target), str(src))
        return f"index update failed after rename: {e}"
    return (path_before, path_after)


def _values_match(current: Any, expected: Any) -> bool:
    if current == expected:
        return True
    # Treat empty string and None as the same "absent" value — tag readers
    # are inconsistent about which they produce.
    return (current in ("", None)) and (expected in ("", None))


def _apply_op(db: DbSession, op: CleanupOpIn) -> dict[str, Any] | str:
    """Apply one accepted operation. Returns the journal item on success,
    a skip reason string on failure."""
    track = db.get(Track, op.track_id)
    if track is None:
        return "track not found"

    if op.kind == "rename":
        if PurePosixPath(track.path).stem != (op.old or ""):
            return "filename changed since analysis"
        result = _rename_in_place(db, track, str(op.new or ""))
        if isinstance(result, str):
            return result
        path_before, path_after = result
        return {
            "kind": "rename",
            "track_id": track.id,
            "path_before": path_before,
            "path_after": path_after,
        }

    if op.field not in _TAG_FIELDS:
        return f"unsupported tag field: {op.field!r}"
    if not _values_match(getattr(track, op.field), op.old):
        return f"{op.field} changed since analysis"
    abs_path = library_index.to_absolute(track.path)
    if not abs_path.is_file():
        return "source file missing on disk"
    # `old` is the row value (may be a filename/folder fallback); the
    # file's REAL pre-write tag goes to the journal too, so revert can
    # restore the exact tag state — deleting a tag that wasn't there
    # instead of materialising the fallback as an explicit tag.
    file_old = library_index.read_file_tags(abs_path).get(op.field)
    try:
        library_index.write_tags(abs_path, {op.field: op.new})
    except ValueError as e:
        return f"unsupported format: {e}"
    except Exception as e:
        return f"tag write failed: {e}"
    library_index.scan_paths(db, [abs_path])
    return {
        "kind": "tag",
        "track_id": track.id,
        "field": op.field,
        "old": op.old,
        "file_old": file_old,
        "new": op.new,
        "path": track.path,
    }


@router.post("/apply", response_model=ApplyResult)
def apply_cleanup(payload: ApplyRequest, _: CurrentUser, db: DbSession) -> ApplyResult:
    batch: CleanupBatch | None = None
    if payload.batch_id is not None:
        batch = db.get(CleanupBatch, payload.batch_id)
        if batch is None:
            raise HTTPException(status_code=status.HTTP_404_NOT_FOUND, detail="batch not found")
        if batch.reverted_at is not None:
            raise HTTPException(
                status_code=status.HTTP_409_CONFLICT,
                detail="batch was already reverted; start a new cleanup run",
            )

    applied_items: list[dict[str, Any]] = []
    skipped: list[CleanupSkip] = []
    for op in payload.ops:
        outcome = _apply_op(db, op)
        if isinstance(outcome, str):
            skipped.append(CleanupSkip(track_id=op.track_id, reason=outcome))
        else:
            applied_items.append(outcome)

    if applied_items:
        if batch is None:
            batch = CleanupBatch(scope_label=payload.scope_label)
            db.add(batch)
            db.flush()
        items: list[dict[str, Any]] = json.loads(batch.items_json)
        items.extend(applied_items)
        batch.items_json = json.dumps(items)
        db.commit()

    return ApplyResult(
        batch_id=batch.id if batch is not None else None,
        applied=len(applied_items),
        skipped=skipped,
    )


# --- journal + revert ---------------------------------------------------------


@router.get("/batches", response_model=list[BatchSummary])
def list_batches(_: CurrentUser, db: DbSession) -> list[BatchSummary]:
    rows = db.scalars(
        select(CleanupBatch).order_by(CleanupBatch.created_at.desc(), CleanupBatch.id.desc()).limit(100)
    ).all()
    return [
        BatchSummary(
            id=b.id,
            created_at=b.created_at,
            scope_label=b.scope_label,
            item_count=len(json.loads(b.items_json)),
            reverted_at=b.reverted_at,
        )
        for b in rows
    ]


def _batch_or_404(db: DbSession, batch_id: int) -> CleanupBatch:
    batch = db.get(CleanupBatch, batch_id)
    if batch is None:
        raise HTTPException(status_code=status.HTTP_404_NOT_FOUND, detail="batch not found")
    return batch


@router.get("/batches/{batch_id}", response_model=BatchDetail)
def get_batch(batch_id: int, _: CurrentUser, db: DbSession) -> BatchDetail:
    b = _batch_or_404(db, batch_id)
    return BatchDetail(
        id=b.id,
        created_at=b.created_at,
        scope_label=b.scope_label,
        item_count=len(json.loads(b.items_json)),
        reverted_at=b.reverted_at,
        items=json.loads(b.items_json),
    )


def _track_for_item(db: DbSession, item: dict[str, Any], current_path: str) -> Track | None:
    """Resolve the journal item's track: by id first, by path as fallback —
    after a DB wipe + rescan the ids are re-minted but paths survive."""
    track_id = item.get("track_id")
    if isinstance(track_id, int):
        track = db.get(Track, track_id)
        if track is not None and track.path == current_path:
            return track
    return db.scalar(select(Track).where(Track.path == current_path))


def _revert_item(db: DbSession, item: dict[str, Any]) -> str | None:
    """Apply the inverse of one journal item. None on success, else a skip
    reason. Drifted state (the file was renamed/re-tagged again after the
    batch) is skipped, never clobbered."""
    kind = item.get("kind")

    if kind == "rename":
        path_before, path_after = item.get("path_before"), item.get("path_after")
        if not isinstance(path_before, str) or not isinstance(path_after, str):
            return "malformed journal item"
        track = _track_for_item(db, item, path_after)
        if track is None:
            return "no track at the recorded path (renamed or removed since)"
        src = library_index.to_absolute(track.path)
        if not src.is_file():
            return "file missing on disk"
        target = library_index.to_absolute(path_before)
        case_only = (
            target.parent == src.parent and target.name.casefold() == src.name.casefold()
        )
        if target.exists() and not case_only:
            return f"original name {Path(path_before).name} is taken"
        shutil.move(str(src), str(target))
        try:
            library_index.update_path(db, track.path, path_before)
        except Exception as e:
            with contextlib.suppress(Exception):
                shutil.move(str(target), str(src))
            return f"index update failed: {e}"
        return None

    if kind == "tag":
        field, path = item.get("field"), item.get("path")
        if not isinstance(field, str) or field not in _TAG_FIELDS or not isinstance(path, str):
            return "malformed journal item"
        track = _track_for_item(db, item, path)
        if track is None:
            return "no track at the recorded path (moved or removed since)"
        if not _values_match(getattr(track, field), item.get("new")):
            return f"{field} changed since this batch was applied"
        abs_path = library_index.to_absolute(track.path)
        if not abs_path.is_file():
            return "file missing on disk"
        # Prefer the file's real pre-write value (`file_old`) so the tag
        # state round-trips exactly — None/"" deletes the tag, returning an
        # originally-untagged file to untagged. `old` (the visible row
        # value) is the fallback for journals predating `file_old`.
        restore = item["file_old"] if "file_old" in item else item.get("old")
        try:
            library_index.write_tags(abs_path, {field: restore})
        except Exception as e:
            return f"tag write failed: {e}"
        library_index.scan_paths(db, [abs_path])
        return None

    return f"unknown journal item kind: {kind!r}"


def _revert_items(db: DbSession, items: list[dict[str, Any]]) -> RevertResult:
    reverted = 0
    skipped: list[CleanupSkip] = []
    # Reverse order: within a track the batch applied tags then rename, so
    # the inverse un-renames first and the tag items' recorded paths line
    # up with where the file is again.
    for item in reversed(items):
        reason = _revert_item(db, item)
        if reason is None:
            reverted += 1
        else:
            track_id = item.get("track_id")
            skipped.append(
                CleanupSkip(
                    track_id=track_id if isinstance(track_id, int) else 0,
                    reason=reason,
                )
            )
    return RevertResult(reverted=reverted, skipped=skipped)


@router.post("/batches/{batch_id}/revert", response_model=RevertResult)
def revert_batch(batch_id: int, _: CurrentUser, db: DbSession) -> RevertResult:
    batch = _batch_or_404(db, batch_id)
    if batch.reverted_at is not None:
        raise HTTPException(
            status_code=status.HTTP_409_CONFLICT, detail="batch already reverted"
        )
    result = _revert_items(db, json.loads(batch.items_json))
    # Marked reverted even when some items skipped — the skips are surfaced
    # to the operator and re-running would just re-skip them; the journal
    # stays downloadable for manual follow-up.
    batch.reverted_at = utcnow()
    db.commit()
    return result


@router.post("/revert", response_model=RevertResult)
def revert_from_journal(
    payload: RevertJournalRequest, _: CurrentUser, db: DbSession
) -> RevertResult:
    """Revert from an uploaded journal (the downloaded batch JSON). Exists
    for the disaster path — app.db was wiped/rebuilt so the server-side
    batch row is gone, but journal items carry paths and survive re-minted
    track ids."""
    return _revert_items(db, payload.items)
