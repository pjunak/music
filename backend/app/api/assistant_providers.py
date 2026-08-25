"""Authenticated configuration endpoints for optional Assistant providers."""

from functools import partial
from time import monotonic
from typing import NoReturn

from fastapi import APIRouter, HTTPException, Request
from starlette.concurrency import run_in_threadpool

from app.api.deps import CurrentUser, DbSession
from app.assistant.model_evaluation import (
    evaluation_job_parameters,
    failed_scenario_job_parameters,
    list_role_evaluations,
)
from app.assistant.providers.execution import (
    CONFORMANCE_CONTRACT,
    run_provider_conformance,
)
from app.assistant.providers.schemas import (
    ModelConformanceOut,
    ModelQualityEvaluationOut,
    ModelRoleOut,
    ModelRoleUpdate,
    ProviderConnectionCreate,
    ProviderConnectionOut,
    ProviderConnectionUpdate,
    ProviderCredentialStorageReset,
    ProviderCredentialStorageResetOut,
    ProviderFrameworkStatusOut,
    ProviderVerificationOut,
)
from app.assistant.providers.service import (
    ProviderServiceError,
    create_connection,
    delete_connection,
    delete_connection_credential,
    delete_model_role,
    finish_role_conformance,
    finish_verification,
    framework_status,
    initialize_provider_credential_storage,
    list_connections,
    list_model_roles,
    prepare_role_conformance,
    prepare_verification,
    reset_provider_credential_storage,
    update_connection,
    update_model_role,
)
from app.assistant.providers.verification import verify_provider_connection
from app.core.password_attempts import password_attempt_throttle
from app.core.security import verify_password
from app.jobs.runner import job_runner
from app.jobs.schemas import BackgroundJobOut, job_out
from app.jobs.service import enqueue_unique_active_job

router = APIRouter(prefix="/api/assistant/providers", tags=["assistant"])


def _raise_http(error: ProviderServiceError) -> NoReturn:
    raise HTTPException(
        status_code=error.status_code,
        detail={"code": error.code, "message": error.message},
    ) from None


@router.get("/status", response_model=ProviderFrameworkStatusOut)
def get_framework_status(
    _user: CurrentUser,
    db: DbSession,
) -> ProviderFrameworkStatusOut:
    return framework_status(db)


@router.post(
    "/credential-storage/initialize",
    response_model=ProviderFrameworkStatusOut,
    status_code=201,
)
def initialize_provider_storage(
    _user: CurrentUser,
    db: DbSession,
) -> ProviderFrameworkStatusOut:
    try:
        return initialize_provider_credential_storage(db)
    except ProviderServiceError as exc:
        _raise_http(exc)


@router.post(
    "/credential-storage/reset",
    response_model=ProviderCredentialStorageResetOut,
)
def reset_provider_storage(
    payload: ProviderCredentialStorageReset,
    request: Request,
    user: CurrentUser,
    db: DbSession,
) -> ProviderCredentialStorageResetOut:
    throttle_key = request.client.host if request.client else "unknown"
    if password_attempt_throttle.blocked(throttle_key):
        raise HTTPException(
            status_code=429,
            detail={
                "code": "password_confirmation_throttled",
                "message": "Too many password attempts; try again shortly.",
            },
        )
    if not verify_password(
        user.password_hash,
        payload.current_password.get_secret_value(),
    ):
        password_attempt_throttle.record_failure(throttle_key)
        raise HTTPException(
            status_code=403,
            detail={
                "code": "current_password_invalid",
                "message": "The current account password is incorrect.",
            },
        )
    password_attempt_throttle.record_success(throttle_key)
    try:
        return reset_provider_credential_storage(db)
    except ProviderServiceError as exc:
        _raise_http(exc)


@router.get("/connections", response_model=list[ProviderConnectionOut])
def get_connections(
    _user: CurrentUser,
    db: DbSession,
) -> list[ProviderConnectionOut]:
    return list_connections(db)


@router.post(
    "/connections",
    response_model=ProviderConnectionOut,
    status_code=201,
)
def add_connection(
    payload: ProviderConnectionCreate,
    _user: CurrentUser,
    db: DbSession,
) -> ProviderConnectionOut:
    try:
        return create_connection(db, payload)
    except ProviderServiceError as exc:
        _raise_http(exc)


@router.put("/connections/{connection_id}", response_model=ProviderConnectionOut)
def edit_connection(
    connection_id: str,
    payload: ProviderConnectionUpdate,
    _user: CurrentUser,
    db: DbSession,
) -> ProviderConnectionOut:
    try:
        return update_connection(db, connection_id, payload)
    except ProviderServiceError as exc:
        _raise_http(exc)


@router.delete("/connections/{connection_id}", status_code=204)
def remove_connection(
    connection_id: str,
    _user: CurrentUser,
    db: DbSession,
) -> None:
    try:
        delete_connection(db, connection_id)
    except ProviderServiceError as exc:
        _raise_http(exc)


@router.delete(
    "/connections/{connection_id}/credential",
    response_model=ProviderConnectionOut,
)
def remove_connection_credential(
    connection_id: str,
    _user: CurrentUser,
    db: DbSession,
) -> ProviderConnectionOut:
    try:
        return delete_connection_credential(db, connection_id)
    except ProviderServiceError as exc:
        _raise_http(exc)


@router.post(
    "/connections/{connection_id}/verify",
    response_model=ProviderVerificationOut,
)
async def verify_connection(
    connection_id: str,
    _user: CurrentUser,
    db: DbSession,
) -> ProviderVerificationOut:
    try:
        target = prepare_verification(db, connection_id)
    except ProviderServiceError as exc:
        _raise_http(exc)

    # DNS, TLS, and the provider request are deliberately moved off the event
    # loop. The verifier also enforces its own timeout and response-size cap.
    result = await run_in_threadpool(
        partial(
            verify_provider_connection,
            target.adapter_id,
            target.base_url,
            target.api_key,
            allow_private_network=target.allow_private_network,
        )
    )
    try:
        connection = finish_verification(db, target, result)
    except ProviderServiceError as exc:
        _raise_http(exc)
    return ProviderVerificationOut(
        connection=connection,
        verified=result.verified,
        error_code=result.error_code,
        models=list(result.models),
    )


@router.get("/roles", response_model=list[ModelRoleOut])
def get_roles(_user: CurrentUser, db: DbSession) -> list[ModelRoleOut]:
    return list_model_roles(db)


@router.get(
    "/roles/{role_id}/evaluations",
    response_model=list[ModelQualityEvaluationOut],
)
def get_role_evaluations(
    role_id: str,
    _user: CurrentUser,
    db: DbSession,
) -> list[ModelQualityEvaluationOut]:
    try:
        return list_role_evaluations(db, role_id)
    except ProviderServiceError as exc:
        _raise_http(exc)


@router.post(
    "/roles/{role_id}/evaluations/{evaluation_id}/jobs",
    response_model=BackgroundJobOut,
    status_code=202,
)
def start_role_evaluation(
    role_id: str,
    evaluation_id: str,
    _user: CurrentUser,
    db: DbSession,
) -> BackgroundJobOut:
    try:
        definition, parameters = evaluation_job_parameters(
            db,
            role_id,
            evaluation_id,
        )
    except ProviderServiceError as exc:
        _raise_http(exc)
    job, created = enqueue_unique_active_job(
        db,
        definition.job_kind,
        parameters,
    )
    if created:
        job_runner.wake()
    return job_out(job)


@router.post(
    "/roles/{role_id}/evaluations/{evaluation_id}/failed-scenarios/jobs",
    response_model=BackgroundJobOut,
    status_code=202,
)
def start_failed_scenario_evaluation(
    role_id: str,
    evaluation_id: str,
    _user: CurrentUser,
    db: DbSession,
) -> BackgroundJobOut:
    try:
        definition, parameters = failed_scenario_job_parameters(
            db,
            role_id,
            evaluation_id,
        )
    except ProviderServiceError as exc:
        _raise_http(exc)
    job, created = enqueue_unique_active_job(
        db,
        definition.job_kind,
        parameters,
    )
    if created:
        job_runner.wake()
    return job_out(job)


@router.put("/roles/{role_id}", response_model=ModelRoleOut)
def set_role(
    role_id: str,
    payload: ModelRoleUpdate,
    _user: CurrentUser,
    db: DbSession,
) -> ModelRoleOut:
    try:
        return update_model_role(db, role_id, payload)
    except ProviderServiceError as exc:
        _raise_http(exc)


@router.delete("/roles/{role_id}", status_code=204)
def remove_role(role_id: str, _user: CurrentUser, db: DbSession) -> None:
    try:
        delete_model_role(db, role_id)
    except ProviderServiceError as exc:
        _raise_http(exc)


@router.post("/roles/{role_id}/test", response_model=ModelConformanceOut)
async def test_role_model(
    role_id: str,
    _user: CurrentUser,
    db: DbSession,
) -> ModelConformanceOut:
    try:
        target = prepare_role_conformance(db, role_id)
    except ProviderServiceError as exc:
        _raise_http(exc)

    # The challenge contains synthetic data only. Network and model work stays
    # off the event loop and uses the same bounded transport as verification.
    started_at = monotonic()
    result = await run_in_threadpool(
        partial(
            run_provider_conformance,
            target.execution,
            target.challenge,
        )
    )
    duration_ms = max(0, round((monotonic() - started_at) * 1000))
    try:
        role = finish_role_conformance(db, target, result)
    except ProviderServiceError as exc:
        _raise_http(exc)
    return ModelConformanceOut(
        role=role,
        passed=result.passed,
        error_code=result.error_code,
        contract_version=CONFORMANCE_CONTRACT,
        provider_model_id=result.provider_model_id,
        finish_reason=result.finish_reason,
        input_tokens=result.input_tokens,
        output_tokens=result.output_tokens,
        duration_ms=duration_ms,
    )
