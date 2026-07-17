"""The custom dispatch scheduler (ADR-0004): direct coroutine stepping.

Handlers run without an ``asyncio.Task``, so the semantics that Task provides
for free are covered explicitly here: contextvars propagation, cancellation
(client disconnect unwinds ``finally`` blocks), Task-protocol compatibility
(``asyncio.current_task``, ``asyncio.all_tasks``, ``asyncio.timeout``,
``TaskGroup``, done callbacks), and graceful-shutdown draining.
"""

import asyncio
import contextlib
import functools
import inspect
import socket
import struct
import sys
import threading
import time
from collections.abc import Callable, Coroutine, Iterator
from contextvars import ContextVar
from typing import Any

import pytest
from rayo import Rayo
from rayo._core import Server
from support import get_json

request_marker: ContextVar[str] = ContextVar("request_marker", default="unset")

# Written by handlers, read back over probe endpoints; keyed by request id so
# tests never see each other's entries.
CANCELLATION_RECORDS: dict[str, dict[str, Any]] = {}
DONE_CALLBACK_RECORDS: dict[str, str] = {}
DECORATOR_OBSERVATIONS: dict[str, bool] = {}

SUPPORTS_TIMEOUT_AND_TASKGROUP = sys.version_info >= (3, 11)
SUPPORTS_MARKCOROUTINEFUNCTION = sys.version_info >= (3, 12)


@pytest.fixture(scope="module")
def running_server() -> Iterator[Server]:
    app = Rayo(title="scheduler test")

    @app.get("/task-identity")
    async def task_identity() -> dict[str, object]:
        current = asyncio.current_task()
        assert current is not None
        current.set_name("scheduler-test-task")
        return {
            "is_asyncio_task": isinstance(current, asyncio.Task),
            "task_type": type(current).__name__,
            "name": current.get_name(),
            "registered_in_all_tasks": current in asyncio.all_tasks(),
            "has_coro": current.get_coro() is not None,
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

    @app.get("/done-callback/{request_id}")
    async def done_callback(request_id: str) -> dict[str, bool]:
        current = asyncio.current_task()
        assert current is not None

        def record_completion(finished_task: "asyncio.Task[object]") -> None:
            DONE_CALLBACK_RECORDS[request_id] = type(finished_task).__name__

        current.add_done_callback(record_completion)
        return {"registered": True}

    @app.get("/cross-loop-await")
    async def cross_loop_await() -> dict[str, bool]:
        foreign_loop = asyncio.new_event_loop()
        try:
            foreign_future: asyncio.Future[object] = foreign_loop.create_future()
            try:
                await foreign_future
            except RuntimeError as loop_error:
                return {"rejected": "different event loop" in str(loop_error)}
            return {"rejected": False}
        finally:
            foreign_loop.close()

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

    if SUPPORTS_MARKCOROUTINEFUNCTION:

        def loop_binding_decorator(
            handler: Callable[..., Coroutine[Any, Any, dict[str, bool]]],
        ) -> Callable[..., Coroutine[Any, Any, dict[str, bool]]]:
            @functools.wraps(handler)
            def wrapper(**handler_kwargs: object) -> Coroutine[Any, Any, dict[str, bool]]:
                # Runs before the coroutine exists — only works when the
                # scheduler builds the handler call on the event-loop thread.
                DECORATOR_OBSERVATIONS["saw_running_loop"] = asyncio.get_running_loop() is not None
                return handler(**handler_kwargs)

            return inspect.markcoroutinefunction(wrapper)

        @app.get("/decorated")
        @loop_binding_decorator
        async def decorated() -> dict[str, bool]:
            return {"wrapper_saw_loop": DECORATOR_OBSERVATIONS.get("saw_running_loop", False)}

    server = app._start(port=0, loop_threads=1)
    yield server
    server.shutdown()


def test_handlers_run_without_an_asyncio_task(running_server: Server) -> None:
    identity = get_json(running_server.port, "/task-identity")
    assert identity["is_asyncio_task"] is False, "dispatch must not create an asyncio.Task"
    assert identity["task_type"] == "HandlerTask"


def test_task_protocol_surface(running_server: Server) -> None:
    identity = get_json(running_server.port, "/task-identity")
    assert identity["name"] == "scheduler-test-task"
    assert identity["registered_in_all_tasks"] is True, (
        "handler tasks must be visible to asyncio.all_tasks()"
    )
    assert identity["has_coro"] is True


def test_contextvars_persist_across_both_resume_paths(running_server: Server) -> None:
    marker = get_json(running_server.port, "/context-marker/alpha")
    assert marker == {"first": "alpha", "second": "alpha"}


def test_contextvars_are_isolated_between_concurrent_requests(running_server: Server) -> None:
    from concurrent.futures import ThreadPoolExecutor

    markers = [f"request-{index}" for index in range(8)]
    with ThreadPoolExecutor(max_workers=len(markers)) as request_pool:
        results = list(
            request_pool.map(
                lambda marker: get_json(running_server.port, f"/context-marker/{marker}"),
                markers,
            )
        )
    assert [result["second"] for result in results] == markers


def test_gather_works_inside_handlers(running_server: Server) -> None:
    assert get_json(running_server.port, "/gather") == {"values": [1, 2, 3]}


def test_done_callbacks_fire_after_completion(running_server: Server) -> None:
    request_id = "done-callback-test"
    assert get_json(running_server.port, f"/done-callback/{request_id}") == {"registered": True}
    callback_deadline = time.perf_counter() + 5
    while request_id not in DONE_CALLBACK_RECORDS and time.perf_counter() < callback_deadline:
        time.sleep(0.01)
    assert DONE_CALLBACK_RECORDS.get(request_id) == "HandlerTask"


def test_cross_loop_awaits_are_rejected_with_a_specific_error(running_server: Server) -> None:
    assert get_json(running_server.port, "/cross-loop-await") == {"rejected": True}


@pytest.mark.skipif(
    not SUPPORTS_TIMEOUT_AND_TASKGROUP, reason="asyncio.timeout requires Python 3.11+"
)
def test_asyncio_timeout_cancels_and_recovers(running_server: Server) -> None:
    assert get_json(running_server.port, "/timeout-guard") == {"timed_out": True}


@pytest.mark.skipif(
    not SUPPORTS_TIMEOUT_AND_TASKGROUP, reason="asyncio.TaskGroup requires Python 3.11+"
)
def test_task_group_completes_inside_handlers(running_server: Server) -> None:
    assert get_json(running_server.port, "/task-group/3/4") == {"total": 14}


@pytest.mark.skipif(
    not SUPPORTS_TIMEOUT_AND_TASKGROUP, reason="asyncio.TaskGroup requires Python 3.11+"
)
def test_task_group_child_failure_surfaces_as_exception_group(running_server: Server) -> None:
    failure = get_json(running_server.port, "/task-group-failure")
    assert failure["caught"] in {"ExceptionGroup", "BaseExceptionGroup"}
    assert failure["leaves"] == ["ValueError"]


@pytest.mark.skipif(
    not SUPPORTS_MARKCOROUTINEFUNCTION,
    reason="inspect.markcoroutinefunction requires Python 3.12+",
)
def test_decorator_wrappers_run_on_the_event_loop_thread(running_server: Server) -> None:
    assert get_json(running_server.port, "/decorated") == {"wrapper_saw_loop": True}


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


def test_shutdown_cancels_parked_handlers_after_grace() -> None:
    app = Rayo(title="shutdown test")
    parked_record: dict[str, bool] = {}

    @app.get("/park")
    async def park() -> dict[str, bool]:
        parked_record["started"] = True
        try:
            await asyncio.sleep(60)
        finally:
            parked_record["cleanup_ran"] = True
        return {"finished": True}

    server = app._start(port=0, loop_threads=1)

    def fire_and_forget() -> None:
        # A 500 or dropped connection is expected after cancellation.
        with contextlib.suppress(Exception):
            get_json(server.port, "/park")

    request_thread = threading.Thread(target=fire_and_forget)
    request_thread.start()
    start_deadline = time.perf_counter() + 5
    while not parked_record.get("started") and time.perf_counter() < start_deadline:
        time.sleep(0.01)
    assert parked_record.get("started"), "handler never started; cannot exercise shutdown"

    shutdown_began = time.perf_counter()
    server.shutdown(grace_seconds=0.2)
    shutdown_elapsed = time.perf_counter() - shutdown_began

    assert parked_record.get("cleanup_ran") is True, (
        "shutdown abandoned a parked handler without running its cleanup"
    )
    assert shutdown_elapsed < 8, (
        f"shutdown took {shutdown_elapsed:.1f}s; the grace period is not being enforced"
    )
    request_thread.join(timeout=5)
