"""Shared helpers for Rayo's test suite."""

import json
import urllib.request


def get_json(port: int, path: str) -> dict[str, object]:
    with urllib.request.urlopen(f"http://127.0.0.1:{port}{path}", timeout=30) as response:
        payload: dict[str, object] = json.loads(response.read())
        return payload
