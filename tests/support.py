"""Shared helpers for Rayo's test suite."""

import json
import time
import urllib.request
from collections.abc import Callable


def get_json(port: int, path: str) -> dict[str, object]:
    with urllib.request.urlopen(f"http://127.0.0.1:{port}{path}", timeout=30) as response:
        payload: dict[str, object] = json.loads(response.read())
        return payload


def wait_until(
    condition: Callable[[], bool],
    failure_message: str | Callable[[], str],
    deadline_seconds: float = 5,
) -> None:
    """Poll ``condition`` until it holds or the deadline passes, then fail
    with ``failure_message`` — a timeout is never a silent pass. Pass a
    callable message to capture state as it is at failure time."""
    deadline = time.perf_counter() + deadline_seconds
    while time.perf_counter() < deadline:
        if condition():
            return
        time.sleep(0.01)
    if not condition():
        message = failure_message() if callable(failure_message) else failure_message
        raise AssertionError(message)
