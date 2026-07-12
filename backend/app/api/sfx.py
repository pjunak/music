"""SFX library HTTP surface.

SFX assets live under SFX_LIBRARY_DIR (separate from the music library).
Mode soundboards reference these files by path relative to that root.

Two surfaces here:

1. Playback path: `GET /api/sfx/file?path=...` — gates access to files that
   are referenced by some loaded soundboard. This protects the operator
   from arbitrary file fetches even if a path passes traversal validation.

2. Management path: `tree`, `upload`, `move`, `delete`, plus folder ops.
   Any file under SFX_LIBRARY_DIR is fair game for these — the operator
   *is* the authority on what lives there.
"""
from __future__ import annotations

import shutil
from datetime import datetime
from pathlib import Path, PurePosixPath
from typing import Annotated, Literal
from uuid import uuid4

from fastapi import APIRouter, File, HTTPException, Query, UploadFile, status
from fastapi.concurrency import run_in_threadpool
from fastapi.responses import FileResponse
from pydantic import BaseModel, Field

from app.api.deps import CurrentUser, OptionalUser
from app.core.config import get_settings
from app.library import index as library_index
from app.modes import loader as modes_loader

router = APIRouter(prefix="/api/sfx", tags=["sfx"])

# What we'll list / upload as SFX. Wider than music — short clips often
# come as wav or ogg from sound packs.
_SFX_EXTENSIONS = frozenset({".mp3", ".wav", ".ogg", ".opus", ".m4a", ".aac", ".flac"})


def sfx_root() -> Path:
    """The configured SFX directory; mkdir on demand for the same fresh-deploy
    parity reasons as `music_root()`."""
    root = get_settings().sfx_library_dir.resolve()
    root.mkdir(parents=True, exist_ok=True)
    return root


def _normalise(rel: str) -> str:
    parts = PurePosixPath(rel.replace("\\", "/")).parts
    if any(p in ("..", "") for p in parts):
        raise ValueError(f"invalid path: {rel!r}")
    return "/".join(parts)


def _any_soundboard_references(rel_path: str) -> bool:
    for manifest in modes_loader.all_modes().values():
        for soundboard in manifest.soundboards.values():
            for category in soundboard.categories:
                for item in category.items:
                    if item.file == rel_path:
                        return True
    return False


# --- response models ------------------------------------------------------


class SfxFileOut(BaseModel):
    name: str
    path: str
    size_bytes: int
    modified_at: datetime
    referenced: bool = Field(
        description="True iff some loaded soundboard has an item with this file path."
    )


class SfxFolderOut(BaseModel):
    name: str
    path: str
    file_count: int
    has_children: bool


class SfxTreeResponse(BaseModel):
    path: str
    files: list[SfxFileOut]


class SfxFoldersResponse(BaseModel):
    folders: list[SfxFolderOut]


class SfxUploadResult(BaseModel):
    saved: list[SfxFileOut]
    destination: str
    # Names skipped because they already existed and conflict="skip" was set.
    skipped: list[str] = Field(default_factory=list)


# How an upload handles a filename that already exists at the destination.
UploadConflict = Literal["rename", "overwrite", "skip"]


class UploadCheckItem(BaseModel):
    dest: str
    name: str


class UploadCheckRequest(BaseModel):
    items: list[UploadCheckItem]


class UploadCheckResponse(BaseModel):
    collisions: list[UploadCheckItem]


class SfxMoveRequest(BaseModel):
    src: str = Field(min_length=1, description="Source path relative to SFX_LIBRARY_DIR")
    dst_folder: str = Field(description="Destination folder; '' for root")
    new_filename: str | None = None


class FolderCreateRequest(BaseModel):
    path: str = Field(min_length=1, description="Folder path relative to SFX_LIBRARY_DIR")


class FolderRenameRequest(BaseModel):
    src: str = Field(min_length=1)
    dst: str = Field(min_length=1)


class FolderDeleteResult(BaseModel):
    deleted: bool


# --- helpers --------------------------------------------------------------


def _stat_file(abs_path: Path, root: Path) -> SfxFileOut:
    s = abs_path.stat()
    rel = library_index.to_relative(abs_path, root)
    return SfxFileOut(
        name=abs_path.name,
        path=rel,
        size_bytes=s.st_size,
        modified_at=datetime.fromtimestamp(s.st_mtime).astimezone(),
        referenced=_any_soundboard_references(rel),
    )


def _count_files_recursive(folder: Path) -> int:
    count = 0
    for child in folder.iterdir():
        if child.is_dir():
            count += _count_files_recursive(child)
        elif child.suffix.lower() in _SFX_EXTENSIONS:
            count += 1
    return count


# --- playback surface (existing contract) --------------------------------


@router.get("/file")
def get_sfx_file(
    _: OptionalUser,
    path: str = Query(..., description="Path relative to SFX_LIBRARY_DIR"),
) -> FileResponse:
    """Stream a single SFX asset.

    1. Path must normalise without traversal segments.
    2. Path must resolve inside SFX_LIBRARY_DIR.
    3. Path must be referenced by at least one loaded mode's soundboards.
    """
    try:
        rel = _normalise(path)
    except ValueError as e:
        raise HTTPException(
            status_code=status.HTTP_400_BAD_REQUEST, detail=str(e)
        ) from e

    if not _any_soundboard_references(rel):
        raise HTTPException(
            status_code=status.HTTP_404_NOT_FOUND,
            detail="sfx path not referenced by any soundboard",
        )

    root = sfx_root()
    try:
        target = library_index.to_absolute(rel, root=root)
    except ValueError as e:
        raise HTTPException(
            status_code=status.HTTP_400_BAD_REQUEST, detail="path escapes sfx root"
        ) from e

    if not target.is_file():
        raise HTTPException(
            status_code=status.HTTP_410_GONE, detail="sfx file missing on disk"
        )
    return FileResponse(target, content_disposition_type="inline")


# --- management surface ---------------------------------------------------


@router.get("/files", response_model=list[SfxFileOut])
def list_all_files(_: CurrentUser) -> list[SfxFileOut]:
    """Flat list of every SFX file, recursive. Used by the soundboard editor
    to populate the file picker without round-tripping per folder."""
    root = sfx_root()
    out: list[SfxFileOut] = []

    def walk(folder: Path) -> None:
        for entry in sorted(folder.iterdir(), key=lambda p: p.name.lower()):
            if entry.is_dir():
                walk(entry)
            elif entry.suffix.lower() in _SFX_EXTENSIONS:
                out.append(_stat_file(entry, root))

    if root.is_dir():
        walk(root)
    return out


@router.get("/folders", response_model=SfxFoldersResponse)
def list_all_folders(_: CurrentUser) -> SfxFoldersResponse:
    """Whole folder hierarchy in one response — same contract as the music
    library's GET /api/library/folders, for the client-side tree."""
    root = sfx_root()
    folders: list[SfxFolderOut] = []

    def walk(abs_dir: Path, rel: str) -> int:
        """Emit subfolders of `abs_dir` depth-first; return its recursive
        audio-file count (what the per-folder badges show)."""
        total = 0
        for child in sorted(abs_dir.iterdir(), key=lambda p: p.name.lower()):
            if child.is_dir():
                child_rel = f"{rel}/{child.name}" if rel else child.name
                child_count = walk(child, child_rel)
                folders.append(
                    SfxFolderOut(
                        name=child.name,
                        path=child_rel,
                        file_count=child_count,
                        has_children=any(g.is_dir() for g in child.iterdir()),
                    )
                )
                total += child_count
            elif child.suffix.lower() in _SFX_EXTENSIONS:
                total += 1
        return total

    if root.is_dir():
        walk(root, "")
    return SfxFoldersResponse(folders=folders)


@router.get("/tree", response_model=SfxTreeResponse)
def list_tree(
    _: CurrentUser,
    path: str = Query("", description="Folder path relative to SFX_LIBRARY_DIR"),
) -> SfxTreeResponse:
    """The SFX files immediately in this folder. The folder hierarchy comes
    from `/folders` (one whole-tree response) — the client builds the tree
    from that, so this endpoint returns only the file list."""
    root = sfx_root()
    rel_clean = path.strip("/").replace("\\", "/")
    if rel_clean:
        try:
            abs_dir = library_index.to_absolute(rel_clean, root=root)
        except ValueError as e:
            raise HTTPException(status_code=status.HTTP_400_BAD_REQUEST, detail="path escapes sfx root") from e
    else:
        abs_dir = root
    if not abs_dir.is_dir():
        return SfxTreeResponse(path=rel_clean, files=[])

    files: list[SfxFileOut] = []
    for entry in sorted(abs_dir.iterdir(), key=lambda p: p.name.lower()):
        if entry.is_file() and entry.suffix.lower() in _SFX_EXTENSIONS:
            files.append(_stat_file(entry, root))
    return SfxTreeResponse(path=rel_clean, files=files)


@router.post(
    "/upload", response_model=SfxUploadResult, status_code=status.HTTP_201_CREATED
)
async def upload(
    _: CurrentUser,
    files: Annotated[list[UploadFile], File()],
    dest: str = Query("", description="Destination folder under SFX_LIBRARY_DIR"),
    conflict: Annotated[
        UploadConflict, Query(description="Policy for existing files")
    ] = "rename",
) -> SfxUploadResult:
    if not files:
        raise HTTPException(
            status_code=status.HTTP_400_BAD_REQUEST, detail="no files provided"
        )
    settings = get_settings()
    if len(files) > settings.max_upload_files:
        raise HTTPException(
            status_code=status.HTTP_413_CONTENT_TOO_LARGE,
            detail=f"too many files in one request (max {settings.max_upload_files})",
        )
    root = sfx_root()
    try:
        library_index.ensure_folder(dest, root=root)
    except ValueError as e:
        raise HTTPException(status_code=status.HTTP_400_BAD_REQUEST, detail=str(e)) from e

    saved: list[SfxFileOut] = []
    skipped: list[str] = []
    for upload_file in files:
        if not upload_file.filename:
            continue
        try:
            target = library_index.safe_join(dest, upload_file.filename, root=root)
        except ValueError as e:
            raise HTTPException(
                status_code=status.HTTP_400_BAD_REQUEST, detail=str(e)
            ) from e
        resolved = library_index.resolve_conflict(target, conflict)
        if resolved is None:
            skipped.append(upload_file.filename)
            continue
        target = resolved
        # Threadpooled disk copy + unique partial — same rationale as the
        # library upload.
        partial = target.with_name(f".{target.name}.{uuid4().hex[:8]}.partial")
        try:
            await run_in_threadpool(
                library_index.stream_upload,
                upload_file.file,
                partial,
                target,
                settings.max_upload_file_bytes,
            )
        except library_index.UploadTooLargeError as e:
            raise HTTPException(
                status_code=status.HTTP_413_CONTENT_TOO_LARGE,
                detail=(
                    f"{upload_file.filename} exceeds the max upload "
                    f"size ({settings.max_upload_file_bytes} bytes)"
                ),
            ) from e
        saved.append(_stat_file(target, root))
    return SfxUploadResult(saved=saved, destination=dest.strip("/"), skipped=skipped)


@router.post("/upload/check", response_model=UploadCheckResponse)
def upload_check(payload: UploadCheckRequest, _: CurrentUser) -> UploadCheckResponse:
    """Report which proposed (dest, name) targets already exist under the SFX
    root, so the client can ask about duplicates before sending bytes."""
    root = sfx_root()
    collisions: list[UploadCheckItem] = []
    for item in payload.items:
        try:
            target = library_index.safe_join(item.dest, item.name, root=root)
        except ValueError:
            continue
        if target.exists():
            collisions.append(item)
    return UploadCheckResponse(collisions=collisions)


@router.post("/move", response_model=SfxFileOut)
def move_file(payload: SfxMoveRequest, _: CurrentUser) -> SfxFileOut:
    root = sfx_root()
    try:
        src_rel = _normalise(payload.src)
    except ValueError as e:
        raise HTTPException(status_code=status.HTTP_400_BAD_REQUEST, detail=str(e)) from e
    try:
        src_abs = library_index.to_absolute(src_rel, root=root)
    except ValueError as e:
        raise HTTPException(status_code=status.HTTP_400_BAD_REQUEST, detail="path escapes sfx root") from e
    if not src_abs.is_file():
        raise HTTPException(status_code=status.HTTP_404_NOT_FOUND, detail="source file not found")

    new_name = payload.new_filename or src_abs.name
    try:
        library_index.ensure_folder(payload.dst_folder, root=root)
        target = library_index.safe_join(payload.dst_folder, new_name, root=root)
    except ValueError as e:
        raise HTTPException(status_code=status.HTTP_400_BAD_REQUEST, detail=str(e)) from e

    if target == src_abs:
        return _stat_file(target, root)
    if target.exists():
        raise HTTPException(
            status_code=status.HTTP_409_CONFLICT, detail="target already exists"
        )
    shutil.move(str(src_abs), str(target))
    return _stat_file(target, root)


@router.delete("/files", status_code=status.HTTP_204_NO_CONTENT)
def delete_file(
    _: CurrentUser,
    path: str = Query(..., description="Path relative to SFX_LIBRARY_DIR"),
) -> None:
    root = sfx_root()
    try:
        rel = _normalise(path)
    except ValueError as e:
        raise HTTPException(status_code=status.HTTP_400_BAD_REQUEST, detail=str(e)) from e
    try:
        target = library_index.to_absolute(rel, root=root)
    except ValueError as e:
        raise HTTPException(status_code=status.HTTP_400_BAD_REQUEST, detail="path escapes sfx root") from e
    if not target.is_file():
        raise HTTPException(status_code=status.HTTP_404_NOT_FOUND, detail="file not found")
    target.unlink()


@router.post(
    "/folders", status_code=status.HTTP_201_CREATED, response_model=SfxFolderOut
)
def create_folder(payload: FolderCreateRequest, _: CurrentUser) -> SfxFolderOut:
    root = sfx_root()
    try:
        abs_path = library_index.ensure_folder(payload.path, root=root)
    except ValueError as e:
        raise HTTPException(status_code=status.HTTP_400_BAD_REQUEST, detail=str(e)) from e
    rel = library_index.to_relative(abs_path, root)
    return SfxFolderOut(
        name=abs_path.name, path=rel, file_count=0, has_children=False
    )


@router.delete("/folders", response_model=FolderDeleteResult)
def delete_folder(
    _: CurrentUser,
    path: str = Query(..., description="Folder path relative to SFX_LIBRARY_DIR"),
    recursive: bool = Query(False),
) -> FolderDeleteResult:
    try:
        library_index.delete_folder(None, path, recursive=recursive, root=sfx_root())
    except FileNotFoundError as e:
        raise HTTPException(status_code=status.HTTP_404_NOT_FOUND, detail=str(e)) from e
    except ValueError as e:
        raise HTTPException(status_code=status.HTTP_400_BAD_REQUEST, detail=str(e)) from e
    return FolderDeleteResult(deleted=True)


@router.post("/folders/rename", response_model=SfxFolderOut)
def rename_folder(payload: FolderRenameRequest, _: CurrentUser) -> SfxFolderOut:
    root = sfx_root()
    try:
        library_index.rename_folder(None, payload.src, payload.dst, root=root)
    except FileNotFoundError as e:
        raise HTTPException(status_code=status.HTTP_404_NOT_FOUND, detail=str(e)) from e
    except FileExistsError as e:
        raise HTTPException(status_code=status.HTTP_409_CONFLICT, detail=str(e)) from e
    except ValueError as e:
        raise HTTPException(status_code=status.HTTP_400_BAD_REQUEST, detail=str(e)) from e
    new_abs = (root / payload.dst.strip("/")).resolve()
    has_children = (
        new_abs.is_dir()
        and any(grandchild.is_dir() for grandchild in new_abs.iterdir())
    )
    return SfxFolderOut(
        name=new_abs.name,
        path=payload.dst.strip("/"),
        file_count=_count_files_recursive(new_abs) if new_abs.is_dir() else 0,
        has_children=has_children,
    )
