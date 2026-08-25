from __future__ import annotations

import threading
import time
from typing import Any

from fastapi.testclient import TestClient

from app.jobs.registry import JobExecutionContext, register_job_handler

SUCCESS_KIND = "tests.progress"
CANCEL_KIND = "tests.cancel"
RESTARTABLE_KIND = "tests.restartable"
NON_RESTARTABLE_KIND = "tests.non-restartable"
LOCAL_LANE_KIND = "tests.local-lane"
PROVIDER_LANE_KIND = "tests.provider-lane"

_cancel_started = threading.Event()
_provider_started = threading.Event()
_provider_release = threading.Event()


def _progress_handler(
    context: JobExecutionContext, parameters: dict[str, Any]
) -> dict[str, Any]:
    steps = int(parameters["steps"])
    for current in range(1, steps + 1):
        context.update_progress(
            current,
            steps,
            phase="Testing",
            message=f"Processed {current}",
        )
    return {"processed": steps}


def _cancellable_handler(
    context: JobExecutionContext, _parameters: dict[str, Any]
) -> dict[str, Any]:
    context.checkpoint_result({"processed": 0})
    _cancel_started.set()
    for current in range(1, 1001):
        time.sleep(0.005)
        context.update_progress(current, 1000, phase="Testing cancellation")
    return {"processed": 1000}


def _blocking_provider_handler(
    context: JobExecutionContext, _parameters: dict[str, Any]
) -> dict[str, Any]:
    _provider_started.set()
    if not _provider_release.wait(timeout=3):
        raise TimeoutError("test did not release provider lane")
    context.check_cancelled()
    return {"released": True}


register_job_handler(SUCCESS_KIND, _progress_handler, restartable=True)
register_job_handler(CANCEL_KIND, _cancellable_handler, restartable=True)
register_job_handler(RESTARTABLE_KIND, _progress_handler, restartable=True)
register_job_handler(NON_RESTARTABLE_KIND, _progress_handler, restartable=False)
register_job_handler(LOCAL_LANE_KIND, _progress_handler, restartable=True, lane="local")
register_job_handler(
    PROVIDER_LANE_KIND,
    _blocking_provider_handler,
    restartable=False,
    lane="provider",
)


def _wait_for_status(
    client: TestClient,
    job_id: str,
    expected: set[str],
    *,
    timeout: float = 3.0,
) -> dict[str, Any]:
    deadline = time.monotonic() + timeout
    latest: dict[str, Any] = {}
    while time.monotonic() < deadline:
        response = client.get(f"/api/jobs/{job_id}")
        assert response.status_code == 200, response.text
        latest = response.json()
        if latest["status"] in expected:
            return latest
        time.sleep(0.02)
    raise AssertionError(f"job did not reach {expected}; latest={latest}")


def test_jobs_api_requires_auth(client: TestClient) -> None:
    assert client.get("/api/jobs").status_code == 401
    assert client.get("/api/jobs/missing").status_code == 401


def test_runner_persists_progress_result_and_history(auth_client: TestClient) -> None:
    from app.core.db import SessionLocal
    from app.jobs.runner import job_runner
    from app.jobs.service import enqueue_job

    with SessionLocal() as db:
        job = enqueue_job(db, SUCCESS_KIND, {"steps": 3})
        job_id = job.id
    job_runner.wake()

    finished = _wait_for_status(auth_client, job_id, {"succeeded"})
    assert finished["progress_current"] == 3
    assert finished["progress_total"] == 3
    assert finished["progress_phase"] == "Complete"
    assert finished["result"] == {"processed": 3}
    assert finished["attempts"] == 1
    assert finished["started_at"] is not None
    assert finished["finished_at"] is not None

    listed = auth_client.get("/api/jobs", params={"kind": SUCCESS_KIND})
    assert listed.status_code == 200
    assert [item["id"] for item in listed.json()] == [job_id]


def test_provider_work_does_not_block_local_job_lane(auth_client: TestClient) -> None:
    from app.core.db import SessionLocal
    from app.jobs.runner import job_runner
    from app.jobs.service import enqueue_job

    _provider_started.clear()
    _provider_release.clear()
    provider_id: str | None = None
    try:
        with SessionLocal() as db:
            provider = enqueue_job(db, PROVIDER_LANE_KIND, {})
            provider_id = provider.id
        job_runner.wake("provider")
        assert _provider_started.wait(timeout=2)

        with SessionLocal() as db:
            local = enqueue_job(db, LOCAL_LANE_KIND, {"steps": 1})
            local_id = local.id
        job_runner.wake("local")

        finished = _wait_for_status(
            auth_client,
            local_id,
            {"succeeded"},
            timeout=2,
        )
        assert finished["result"] == {"processed": 1}
    finally:
        _provider_release.set()
        job_runner.wake("provider")

    assert provider_id is not None
    provider_finished = _wait_for_status(
        auth_client,
        provider_id,
        {"succeeded"},
    )
    assert provider_finished["result"] == {"released": True}


def test_running_job_can_be_cancelled_and_retried(auth_client: TestClient) -> None:
    from app.core.db import SessionLocal
    from app.jobs.runner import job_runner
    from app.jobs.service import enqueue_job

    _cancel_started.clear()
    with SessionLocal() as db:
        job = enqueue_job(db, CANCEL_KIND, {})
        job_id = job.id
    job_runner.wake()
    assert _cancel_started.wait(timeout=2)

    cancelled = auth_client.post(f"/api/jobs/{job_id}/cancel")
    assert cancelled.status_code == 200, cancelled.text
    assert cancelled.json()["status"] in {"cancel_requested", "cancelled"}
    terminal = _wait_for_status(auth_client, job_id, {"cancelled"})
    assert terminal["finished_at"] is not None
    assert terminal["result"] == {"processed": 0}

    retried = auth_client.post(f"/api/jobs/{job_id}/retry")
    assert retried.status_code == 200, retried.text
    retry = retried.json()
    assert retry["id"] != job_id
    assert retry["retry_of_id"] == job_id
    assert retry["status"] == "queued"

    # Leave no deliberately slow work behind for TestClient shutdown.
    retry_cancel = auth_client.post(f"/api/jobs/{retry['id']}/cancel")
    assert retry_cancel.status_code == 200, retry_cancel.text
    _wait_for_status(auth_client, retry["id"], {"cancelled"})


def test_restart_recovery_respects_handler_policy(db_session) -> None:
    from app.jobs.runner import BackgroundJobRunner
    from app.models.background_job import BackgroundJob

    rows = [
        BackgroundJob(
            id="a" * 32,
            kind=RESTARTABLE_KIND,
            status="running",
            parameters_json='{"steps":1}',
            progress_current=4,
            progress_total=10,
        ),
        BackgroundJob(
            id="b" * 32,
            kind=NON_RESTARTABLE_KIND,
            status="running",
            parameters_json='{"steps":1}',
        ),
        BackgroundJob(
            id="c" * 32,
            kind=RESTARTABLE_KIND,
            status="cancel_requested",
            parameters_json='{"steps":1}',
        ),
    ]
    db_session.add_all(rows)
    db_session.commit()

    BackgroundJobRunner().recover_interrupted_jobs()
    db_session.expire_all()

    restartable, non_restartable, cancelling = [
        db_session.get(BackgroundJob, row.id) for row in rows
    ]
    assert restartable is not None and restartable.status == "queued"
    assert restartable.progress_current == 4
    assert non_restartable is not None and non_restartable.status == "failed"
    assert non_restartable.error == "Job was interrupted by a server restart."
    assert cancelling is not None and cancelling.status == "cancelled"


def test_graceful_stop_respects_restart_policy_and_keeps_checkpoints(
    db_session,
) -> None:
    from app.jobs.runner import BackgroundJobRunner
    from app.models.background_job import BackgroundJob

    rows = [
        BackgroundJob(
            id="d" * 32,
            kind=RESTARTABLE_KIND,
            status="running",
            parameters_json='{"steps":1}',
            result_json='{"processed":2}',
        ),
        BackgroundJob(
            id="e" * 32,
            kind=NON_RESTARTABLE_KIND,
            status="running",
            parameters_json='{"steps":1}',
            result_json='{"provider_usage":true}',
        ),
    ]
    db_session.add_all(rows)
    db_session.commit()
    runner = BackgroundJobRunner()

    runner._finish_interrupted(rows[0].id, restartable=True)
    runner._finish_interrupted(rows[1].id, restartable=False)
    db_session.expire_all()

    restartable, non_restartable = [
        db_session.get(BackgroundJob, row.id) for row in rows
    ]
    assert restartable is not None and restartable.status == "queued"
    assert restartable.result_json == '{"processed":2}'
    assert non_restartable is not None and non_restartable.status == "failed"
    assert non_restartable.error == "Job was interrupted during server shutdown."
    assert non_restartable.result_json == '{"provider_usage":true}'
