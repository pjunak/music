from datetime import datetime
from pathlib import Path
from typing import Annotated, Literal

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

SortKey = Literal["title", "artist", "album", "album_artist", "year", "length_s", "track_no"]
SortOrder = Literal["asc", "desc"]


class SearchResponse(BaseModel):
    tracks: list[Track]
    total: int
    limit: int
    offset: int
    sort: SortKey
    order: SortOrder


class IncomingFile(BaseModel):
    name: str
    size_bytes: int
    modified_at: datetime


class IncomingListing(BaseModel):
    files: list[IncomingFile]


class UploadResult(BaseModel):
    saved: list[IncomingFile]


class IngestRequest(BaseModel):
    autotag: bool = False


class IngestResponse(BaseModel):
    ok: bool
    returncode: int
    imported: int
    skipped: int
    stdout: str
    stderr: str


def _sort_key(track: Track, key: SortKey) -> tuple:
    """Return a normalized comparison key for sorting. Tuple keeps None
    values consistently sorted to the end without TypeErrors."""
    value = getattr(track, key)
    if isinstance(value, str):
        # Case-insensitive sort, with leading articles ignored for "title-ish" fields.
        normalized = value.lower().strip()
        if key in ("title", "album", "artist", "album_artist"):
            for article in ("the ", "a ", "an "):
                if normalized.startswith(article):
                    normalized = normalized[len(article):]
                    break
        return (0, normalized)
    if value is None:
        return (1, 0)
    return (0, value)


@router.get("/search", response_model=SearchResponse)
def search(
    _: CurrentUser,
    q: str = Query("", description="Beets query DSL (e.g. 'artist:daft year:2001..2003')"),
    limit: int = Query(100, ge=1, le=500),
    offset: int = Query(0, ge=0),
    sort: Annotated[SortKey, Query(description="Sort key")] = "artist",
    order: Annotated[SortOrder, Query(description="Sort direction")] = "asc",
) -> SearchResponse:
    # Pull all matches first (Beets queries are cheap; the library fits in
    # memory for any single-user case), then sort and paginate. Cleaner than
    # mapping our sort keys into Beets' query DSL.
    all_tracks = beets_adapter.search(q)
    all_tracks.sort(key=lambda t: _sort_key(t, sort), reverse=(order == "desc"))
    page = all_tracks[offset : offset + limit]
    return SearchResponse(
        tracks=page,
        total=len(all_tracks),
        limit=limit,
        offset=offset,
        sort=sort,
        order=order,
    )


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
async def trigger_ingest(
    _: CurrentUser, payload: IngestRequest | None = None
) -> IngestResponse:
    """Run `beet import` against the incoming directory.

    Default is no-autotag (``-A -q``): files import as-is using their
    embedded tags, which works for any audio Beets can read. Pass
    ``{"autotag": true}`` to run with MusicBrainz auto-tagging instead —
    that mode skips files that don't get a strong match.

    Bubbles `beet`'s stdout/stderr plus a parsed imported/skipped summary
    back to the caller."""
    incoming = _incoming_dir()
    autotag = payload.autotag if payload is not None else False
    try:
        result = await ingest.run_autoimport(incoming, autotag=autotag)
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
        imported=result.imported,
        skipped=result.skipped,
        stdout=result.stdout,
        stderr=result.stderr,
    )
