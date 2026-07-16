"""The custom dispatch scheduler (ADR-0004): direct coroutine stepping.

Handlers run without an ``asyncio.Task``, so the semantics that Task provides
for free are covered explicitly here: contextvars propagation, cancellation
(client disconnect unwinds ``finally`` blocks), and Task-protocol
compatibility (``asyncio.current_task``, ``asyncio.timeout``, ``TaskGroup``).
"""

import asyncio
import json
import socket
import struct
import sys
import time
import urllib.request
from collections.abc import Iterator
from concurrent.futures import ThreadPoolExecutor
from contextvars import ContextVar
from typing import Any

import pytest
from rayo import Rayo
from rayo._core import Server

request_marker: ContextVar[str] = ContextVar("request_marker", default="unset")

# Written by the /slow-with-cleanup handler, read back over /cancellation-record;
# keyed by request id so tests never see each other's entries.
CANCELLATION_RECORDS: dict[str, dict[str, Any]] = {}

SUPPORTS_TIMEOUT_AND_TASKGROUP = sys.version_info >= (3, 11)


def _get_json(port: int, path: str) -> dict[str, object]:
    with urllib.request.urlopen(f"http://127.0.0.1:{port}{path}", timeout=30) as response:
        payload: dict[str, object] = json.loads(response.read())
        return payload


@pytest.fixture(scope="module")
def running_server() -> Iterator[Server]:
    app = Rayo(title="scheduler test")

    @app.get("/task-identity")
    async def task_identity() -> dict[str, object]:
        current = asyncio.current_task()
        return {
            "has_task": current is not None,
            "is_asyncio_task": isinstance(current, asyncio.Task),
            "task_type": type(current).__name__ if current is not None else None,
        }

    @app.get("/context-marker/{marker}")
    async def context_marker(marker: str) -> dict[str, str]:
        request_marker.set(marker)
        await asyncio.sleep(0)  # bare-yield resume (call_soon path)
        first_read = request_marker.get()
        await asyncio.sleep(0.02)  # future-based resume (done-callback path)
        return {"first": first_read, "second": request_marker.get()}

    @app.get("/gather")
    async def gather_handler() -> dict[str, object]:
        async def echo_after_sleep(value: int) -> int:
            await asyncio.sleep(0.01)
            return value

        values = await asyncio.gather(echo_after_sleep(1), echo_after_sleep(2), echo_after_sleep(3))
        return {"values": list(values)}

    @app.get("/slow-with-cleanup/{request_id}")
    async def slow_with_cleanup(request_id: str) -> dict[str, object]:
        record = CANCELLATION_RECORDS.setdefault(request_id, {})
        record["started"] = True
        try:
            await asyncio.sleep(30)
        except asyncio.CancelledError:
            record["cancelled"] = True
            raise
        finally:
            record["cleanup_ran"] = True
        return {"finished": True}

    @app.get("/cancellation-record/{request_id}")
    async def cancellation_record(request_id: str) -> dict[str, object]:
        return {"record": CANCELLATION_RECORDS.get(request_id, {})}

    if SUPPORTS_TIMEOUT_AND_TASKGROUP:

        @app.get("/timeout-guard")
        async def timeout_guard() -> dict[str, bool]:
            try:
                async with asyncio.timeout(0.05):
                    await asyncio.sleep(30)
            except TimeoutError:
                return {"timed_out": True}
            return {"timed_out": False}

        @app.get("/task-group/{left}/{right}")
        async def task_group(left: int, right: int) -> dict[str, int]:
            async def doubled(value: int) -> int:
                await asyncio.sleep(0.01)
                return value * 2

            async with asyncio.TaskGroup() as group:
                left_task = group.create_task(doubled(left))
                right_task = group.create_task(doubled(right))
            return {"total": left_task.result() + right_task.result()}

        @app.get("/task-group-failure")
        async def task_group_failure() -> dict[str, object]:
            async def failing_child() -> None:
                await asyncio.sleep(0.01)
                raise ValueError("child failed")

            async def steady_child() -> None:
                await asyncio.sleep(30)

            try:
                async with asyncio.TaskGroup() as group:
                    group.create_task(failing_child())
                    group.create_task(steady_child())
            except Exception as group_error:
                leaves = getattr(group_error, "exceptions", ())
                return {
                    "caught": type(group_error).__name__,
                    "leaves": sorted(type(leaf).__name__ for leaf in leaves),
                }
            return {"caught": None, "leaves": []}

    server = app._start(port=0, loop_threads=1)
    yield server
    server.shutdown()


def test_handlers_run_without_an_asyncio_task(running_server: Server) -> None:
    identity = _get_json(running_server.port, "/task-identity")
    assert identity["has_task"] is True, "asyncio.current_task() must see the handler task"
    assert identity["is_asyncio_task"] is False, "dispatch must not create an asyncio.Task"
    assert identity["task_type"] == "HandlerTask"


def test_contextvars_persist_across_both_resume_paths(running_server: Server) -> None:
    marker = _get_json(running_server.port, "/context-marker/alpha")
    assert marker == {"first": "alpha", "second": "alpha"}


def test_contextvars_are_isolated_between_concurrent_requests(running_server: Server) -> None:
    markers = [f"request-{index}" for index in range(8)]
    with ThreadPoolExecutor(max_workers=len(markers)) as request_pool:
        results = list(
            request_pool.map(
                lambda marker: _get_json(running_server.port, f"/context-marker/{marker}"),
                markers,
            )
        )
    assert [result["second"] for result in results] == markers


def test_gather_works_inside_handlers(running_server: Server) -> None:
    assert _get_json(running_server.port, "/gather") == {"values": [1, 2, 3]}


@pytest.mark.skipif(
    not SUPPORTS_TIMEOUT_AND_TASKGROUP, reason="asyncio.timeout requires Python 3.11+"
)
def test_asyncio_timeout_cancels_and_recovers(running_server: Server) -> None:
    assert _get_json(running_server.port, "/timeout-guard") == {"timed_out": True}


@pytest.mark.skipif(
    not SUPPORTS_TIMEOUT_AND_TASKGROUP, reason="asyncio.TaskGroup requires Python 3.11+"
)
def test_task_group_completes_inside_handlers(running_server: Server) -> None:
    assert _get_json(running_server.port, "/task-group/3/4") == {"total": 14}


@pytest.mark.skipif(
    not SUPPORTS_TIMEOUT_AND_TASKGROUP, reason="asyncio.TaskGroup requires Python 3.11+"
)
def test_task_group_child_failure_surfaces_as_exception_group(running_server: Server) -> None:
    failure = _get_json(running_server.port, "/task-group-failure")
    assert failure["caught"] in {"ExceptionGroup", "BaseExceptionGroup"}
    assert failure["leaves"] == ["ValueError"]


def test_client_disconnect_cancels_handler_and_runs_cleanup(running_server: Server) -> None:
    request_id = "disconnect-test"

    with socket.create_connection(("127.0.0.1", running_server.port), timeout=5) as connection:
        connection.sendall(
            f"GET /slow-with-cleanup/{request_id} HTTP/1.1\r\nHost: 127.0.0.1\r\n\r\n".encode()
        )
        # Wait until the handler is inside its await before disconnecting.
        start_deadline = time.perf_counter() + 5
        while (
            not CANCELLATION_RECORDS.get(request_id, {}).get("started")
            and time.perf_counter() < start_deadline
        ):
            time.sleep(0.01)
        assert CANCELLATION_RECORDS.get(request_id, {}).get("started"), (
            "handler never started; cannot exercise disconnect cancellation"
        )
        # Reset instead of FIN so the server sees the disconnect immediately.
        connection.setsockopt(socket.SOL_SOCKET, socket.SO_LINGER, struct.pack("ii", 1, 0))

    cancel_deadline = time.perf_counter() + 10
    while (
        not CANCELLATION_RECORDS.get(request_id, {}).get("cleanup_ran")
        and time.perf_counter() < cancel_deadline
    ):
        time.sleep(0.02)
    record = CANCELLATION_RECORDS.get(request_id, {})
    assert record.get("cancelled") is True, f"handler was not cancelled on disconnect: {record}"
    assert record.get("cleanup_ran") is True, f"finally block did not run: {record}"
