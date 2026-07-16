"""Multi-core serving: N event-loop threads in one process.

On free-threaded builds the loop threads run Python handlers truly in
parallel; on GIL builds the same API works with concurrency but not
parallelism. Both properties are covered here.
"""

import json
import sys
import threading
import time
import urllib.request
from collections.abc import Iterator
from concurrent.futures import ThreadPoolExecutor

import pytest
from rayo import Rayo
from rayo._core import Server


def _is_free_threaded() -> bool:
    gil_check = getattr(sys, "_is_gil_enabled", None)
    return gil_check is not None and not gil_check()


def _get_json(port: int, path: str) -> dict[str, object]:
    with urllib.request.urlopen(f"http://127.0.0.1:{port}{path}", timeout=30) as response:
        payload: dict[str, object] = json.loads(response.read())
        return payload


LOOP_THREADS = 4


@pytest.fixture(scope="module")
def running_server() -> Iterator[Server]:
    app = Rayo(title="loop-threads test")

    @app.get("/loop-thread")
    async def loop_thread() -> dict[str, str]:
        return {"thread": threading.current_thread().name}

    @app.get("/burn/{milliseconds}")
    async def burn(milliseconds: int) -> dict[str, object]:
        deadline = time.perf_counter() + milliseconds / 1000
        spin_count = 0
        while time.perf_counter() < deadline:
            spin_count += 1
        return {"thread": threading.current_thread().name, "spins": spin_count}

    server = app._start(port=0, loop_threads=LOOP_THREADS)
    yield server
    server.shutdown()


def test_rejects_zero_loop_threads() -> None:
    app = Rayo(title="invalid config")
    with pytest.raises(ValueError, match="loop_threads must be at least 1"):
        app._start(port=0, loop_threads=0)


def test_in_flight_gauge_drains_to_zero(running_server: Server) -> None:
    with ThreadPoolExecutor(max_workers=8) as request_pool:
        burst_futures = [
            request_pool.submit(_get_json, running_server.port, "/burn/20") for _ in range(16)
        ]
        for burst_future in burst_futures:
            burst_future.result()

    # The gauge decrements on the loop thread as each completion callback
    # runs, which can trail the client seeing its response; poll briefly.
    drain_deadline = time.perf_counter() + 5
    while running_server.in_flight != 0 and time.perf_counter() < drain_deadline:
        time.sleep(0.01)
    assert running_server.in_flight == 0


def test_requests_distribute_across_all_loop_threads(running_server: Server) -> None:
    serving_threads = {
        str(_get_json(running_server.port, "/loop-thread")["thread"])
        for _ in range(LOOP_THREADS * 4)
    }
    assert serving_threads == {f"rayo-loop-{index}" for index in range(LOOP_THREADS)}


@pytest.mark.skipif(
    not _is_free_threaded(),
    reason="true parallelism requires a free-threaded build; GIL builds get concurrency only",
)
def test_cpu_bound_handlers_run_in_parallel_on_free_threaded(running_server: Server) -> None:
    burn_ms = 150

    solo_started = time.perf_counter()
    _get_json(running_server.port, f"/burn/{burn_ms}")
    solo_elapsed = time.perf_counter() - solo_started

    concurrent_requests = LOOP_THREADS
    concurrent_started = time.perf_counter()
    with ThreadPoolExecutor(max_workers=concurrent_requests) as request_pool:
        request_futures = [
            request_pool.submit(_get_json, running_server.port, f"/burn/{burn_ms}")
            for _ in range(concurrent_requests)
        ]
        for request_future in request_futures:
            request_future.result()
    concurrent_elapsed = time.perf_counter() - concurrent_started

    # Serial execution would take ~concurrent_requests x solo. Parallel
    # execution stays near 1x solo; 2.5x leaves generous headroom for
    # noisy CI runners while still ruling out GIL-style serialization.
    assert concurrent_elapsed < solo_elapsed * 2.5, (
        f"{concurrent_requests} concurrent CPU-bound requests took "
        f"{concurrent_elapsed:.3f}s vs {solo_elapsed:.3f}s solo — handlers are "
        f"serializing instead of running in parallel across loop threads"
    )
