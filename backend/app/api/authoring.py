"""Authenticated review-first endpoints for Authoring imports."""
from __future__ import annotations

from typing import Any

from fastapi import APIRouter

from app.api.deps import CurrentUser, DbSession
from app.authoring.schemas import (
    AuthoringDocumentCommitRequest,
    AuthoringDocumentPreviewRequest,
    AuthoringImportCommitRequest,
    AuthoringImportPreview,
    AuthoringImportPreviewRequest,
    AuthoringImportResult,
    public_document_schema,
)
from app.authoring.service import build_preview, commit_bundle, mode_or_404
from app.authoring.sources import ImportBundle, bundle_from_document, bundle_from_mode

router = APIRouter(prefix="/api/authoring/import", tags=["authoring"])


def _mode_bundle(db: DbSession, mode_id: str) -> ImportBundle:
    mode_or_404(mode_id)
    return bundle_from_mode(db, mode_id)


@router.get("/document/schema", response_model=dict[str, Any])
def get_document_schema(_user: CurrentUser) -> dict[str, Any]:
    """Expose the canonical v1 contract to authoring tools and assistants."""

    return public_document_schema()


@router.post("/preview", response_model=AuthoringImportPreview)
def preview_mode_import(
    payload: AuthoringImportPreviewRequest,
    _user: CurrentUser,
    db: DbSession,
) -> AuthoringImportPreview:
    bundle = _mode_bundle(db, payload.source_mode_id)
    return build_preview(db, payload.target_mode_id, bundle)


@router.post("/commit", response_model=AuthoringImportResult)
def commit_mode_import(
    payload: AuthoringImportCommitRequest,
    _user: CurrentUser,
    db: DbSession,
) -> AuthoringImportResult:
    return commit_bundle(
        db,
        payload.target_mode_id,
        lambda: _mode_bundle(db, payload.source_mode_id),
        payload.items,
    )


@router.post("/document/preview", response_model=AuthoringImportPreview)
def preview_document_import(
    payload: AuthoringDocumentPreviewRequest,
    _user: CurrentUser,
    db: DbSession,
) -> AuthoringImportPreview:
    bundle = bundle_from_document(payload.document, payload.source_name)
    return build_preview(db, payload.target_mode_id, bundle)


@router.post("/document/commit", response_model=AuthoringImportResult)
def commit_document_import(
    payload: AuthoringDocumentCommitRequest,
    _user: CurrentUser,
    db: DbSession,
) -> AuthoringImportResult:
    return commit_bundle(
        db,
        payload.target_mode_id,
        lambda: bundle_from_document(payload.document, payload.source_name),
        payload.items,
    )
