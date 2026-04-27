from datetime import datetime
from pathlib import Path
from typing import Annotated

from fastapi import APIRouter, File, HTTPException, Query, UploadFile, status
from fastapi.responses import FileResponse
from pydantic import BaseModel

from app.api.deps import CurrentUser
from app.core.config import get_settings
from app.library import beets_adapter, ingest
from app.library.beets_adapter import Track

router = APIRouter(prefix="/api/library", tags=["library"])

# Streamed upload chunk size. 1 MiB balances memory use against syscall count.
_UPLOAD_CHUNK = 1 << 20


class SearchResponse(BaseModel):
    tracks: list[Track]
    limit: int
    offset: int


class IncomingFile(BaseModel):
    name: str
    size_bytes: int
    modified_at: datetime


class IncomingListing(BaseModel):
    files: list[IncomingFile]


class UploadResult(BaseModel):
    saved: list[IncomingFile]


class IngestResponse(BaseModel):
    ok: bool
    returncode: int
    stdout: str
    stderr: str


@router.get("/search", response_model=SearchResponse)
def search(
    _: CurrentUser,
    q: str = Query("", description="Beets query DSL (e.g. 'artist:daft year:2001..2003')"),
    limit: int = Query(100, ge=1, le=500),
    offset: int = Query(0, ge=0),
) -> SearchResponse:
    tracks = beets_adapter.search(q, limit=limit, offset=offset)
    return SearchResponse(tracks=tracks, limit=limit, offset=offset)


@router.get("/tracks/{beets_id}", response_model=Track)
def get_track(beets_id: int, _: CurrentUser) -> Track:
    track = beets_adapter.get_track(beets_id)
    if track is None:
        raise HTTPException(status_code=status.HTTP_404_NOT_FOUND, detail="track not found")
    return track


@router.get("/tracks/{beets_id}/stream")
def stream_track(beets_id: int, _: CurrentUser) -> FileResponse:
    track = beets_adapter.get_track(beets_id)
    if track is None:
        raise HTTPException(status_code=status.HTTP_404_NOT_FOUND, detail="track not found")
    path = Path(track.path)
    if not path.is_file():
        # Beets DB references a file that's no longer on disk.
        raise HTTPException(status_code=status.HTTP_410_GONE, detail="track file missing")
    return FileResponse(path, content_disposition_type="inline")


# --- Upload manager --------------------------------------------------------
#
# The "incoming/" directory is the staging area for tracks awaiting Beets
# ingestion. Upload writes here; ingest hands the directory to `beet import`,
# which moves successful matches into the Beets-managed library tree.


def _incoming_dir() -> Path:
    """Return the configured incoming dir, creating it on demand. Beets and
    the user can drop files here directly too — we just manage uploads."""
    path = get_settings().incoming_dir
    path.mkdir(parents=True, exist_ok=True)
    return path


def _stat_incoming(path: Path) -> IncomingFile:
    s = path.stat()
    return IncomingFile(
        name=path.name,
        size_bytes=s.st_size,
        modified_at=datetime.fromtimestamp(s.st_mtime).astimezone(),
    )


def _safe_target(name: str, incoming: Path) -> Path:
    """Resolve `name` to a non-colliding path inside `incoming`. Strips any
    directory components from the supplied filename to defeat path traversal,
    and de-duplicates `foo.mp3` → `foo-1.mp3` if a file already exists."""
    base = Path(name).name
    if not base or base in {".", ".."}:
        raise HTTPException(
            status_code=status.HTTP_400_BAD_REQUEST, detail=f"invalid filename: {name!r}"
        )
    target = incoming / base
    if not target.exists():
        return target
    stem, suffix = target.stem, target.suffix
    n = 1
    while True:
        candidate = incoming / f"{stem}-{n}{suffix}"
        if not candidate.exists():
            return candidate
        n += 1


@router.get("/incoming", response_model=IncomingListing)
def list_incoming(_: CurrentUser) -> IncomingListing:
    incoming = _incoming_dir()
    files = sorted(
        (_stat_incoming(p) for p in incoming.iterdir() if p.is_file()),
        key=lambda f: f.name,
    )
    return IncomingListing(files=files)


@router.post(
    "/upload", response_model=UploadResult, status_code=status.HTTP_201_CREATED
)
async def upload(
    _: CurrentUser, files: Annotated[list[UploadFile], File()]
) -> UploadResult:
    if not files:
        raise HTTPException(
            status_code=status.HTTP_400_BAD_REQUEST, detail="no files provided"
        )
    incoming = _incoming_dir()
    saved: list[IncomingFile] = []
    for upload_file in files:
        if not upload_file.filename:
            continue
        target = _safe_target(upload_file.filename, incoming)
        with target.open("wb") as out:
            while chunk := await upload_file.read(_UPLOAD_CHUNK):
                out.write(chunk)
        saved.append(_stat_incoming(target))
    return UploadResult(saved=saved)


@router.delete(
    "/incoming/{filename}", status_code=status.HTTP_204_NO_CONTENT
)
def delete_incoming(filename: str, _: CurrentUser) -> None:
    incoming = _incoming_dir()
    base = Path(filename).name
    if not base or base in {".", ".."}:
        raise HTTPException(
            status_code=status.HTTP_400_BAD_REQUEST, detail=f"invalid filename: {filename!r}"
        )
    target = incoming / base
    if not target.is_file():
        raise HTTPException(status_code=status.HTTP_404_NOT_FOUND, detail="file not found")
    target.unlink()


@router.post("/ingest", response_model=IngestResponse)
async def trigger_ingest(_: CurrentUser) -> IngestResponse:
    """Run `beet import -q` against the incoming directory.

    Strong matches are moved into the Beets library; weak ones stay in
    incoming/ for manual review. Bubbles `beet`'s stdout/stderr back to the
    caller so the operator can see what happened (or why it didn't)."""
    incoming = _incoming_dir()
    try:
        result = await ingest.run_autoimport(incoming)
    except FileNotFoundError as e:
        # `beet` not on PATH, or the dir disappeared between the mkdir above
        # and the subprocess call.
        raise HTTPException(
            status_code=status.HTTP_503_SERVICE_UNAVAILABLE, detail=str(e)
        ) from e
    # A fresh ingest may have created the Beets DB; drop the cached handle so
    # the next search re-opens it.
    beets_adapter.invalidate_cache()
    return IngestResponse(
        ok=result.ok,
        returncode=result.returncode,
        stdout=result.stdout,
        stderr=result.stderr,
    )
