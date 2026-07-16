"""End-to-end tests for the v0 request spine: real server, real HTTP,
async and sync handlers, typed path parameters, and error paths."""

import json
import urllib.error
import urllib.request
from collections.abc import Iterator
from http.client import HTTPResponse
from typing import Any

import pytest
from rayo import Rayo
from rayo._core import Server


def _request(port: int, path: str) -> tuple[int, str, bytes]:
    """GET the path; return (status, content_type, body) without raising on 4xx/5xx."""
    request_url = f"http://127.0.0.1:{port}{path}"
    try:
        response: HTTPResponse = urllib.request.urlopen(request_url, timeout=10)
    except urllib.error.HTTPError as error_response:
        return (
            error_response.code,
            error_response.headers.get("content-type", ""),
            error_response.read(),
        )
    with response:
        return response.status, response.headers.get("content-type", ""), response.read()


@pytest.fixture(scope="module")
def running_server() -> Iterator[Server]:
    app = Rayo(title="spine test")

    @app.get("/hello/{name}")
    async def hello(name: str) -> dict[str, str]:
        return {"message": f"hi {name}"}

    @app.get("/add/{left}/{right}")
    def add(left: int, right: int) -> dict[str, int]:  # sync handler on purpose
        return {"total": left + right}

    @app.get("/motto")
    async def motto() -> str:
        return "the process is the cluster"

    @app.get("/empty")
    async def empty() -> None:
        return None

    @app.get("/boom")
    async def boom() -> None:
        raise RuntimeError("intentional test failure")

    server = app._start(port=0)
    yield server
    server.shutdown()


def test_async_handler_returns_json_with_path_param(running_server: Server) -> None:
    status, content_type, body = _request(running_server.port, "/hello/world")
    assert status == 200
    assert content_type == "application/json"
    assert json.loads(body) == {"message": "hi world"}


def test_sync_handler_with_int_conversion(running_server: Server) -> None:
    status, _content_type, body = _request(running_server.port, "/add/19/23")
    assert status == 200
    assert json.loads(body) == {"total": 42}


def test_str_return_is_plain_text(running_server: Server) -> None:
    status, content_type, body = _request(running_server.port, "/motto")
    assert status == 200
    assert content_type == "text/plain; charset=utf-8"
    assert body == b"the process is the cluster"


def test_none_return_is_204(running_server: Server) -> None:
    status, _content_type, body = _request(running_server.port, "/empty")
    assert status == 204
    assert body == b""


def test_unknown_path_is_404(running_server: Server) -> None:
    status, _content_type, _body = _request(running_server.port, "/nope")
    assert status == 404


def test_invalid_int_param_is_422_naming_the_parameter(running_server: Server) -> None:
    status, content_type, body = _request(running_server.port, "/add/one/2")
    assert status == 422
    assert content_type == "application/json"
    assert json.loads(body) == {"detail": "path parameter 'left' expected int, got 'one'"}


def test_handler_exception_is_opaque_500(running_server: Server) -> None:
    status, _content_type, body = _request(running_server.port, "/boom")
    assert status == 500
    assert body == b"Internal Server Error"
    assert b"intentional" not in body  # tracebacks go to logs, never to clients


def test_handler_with_unknown_argument_fails_at_registration() -> None:
    app = Rayo()
    with pytest.raises(TypeError, match="does not match any path parameter"):

        @app.get("/items/{item_id}")
        async def item(item_id: int, verbose: bool) -> dict[str, Any]:
            return {}


def test_path_param_without_handler_argument_fails_at_registration() -> None:
    app = Rayo()
    with pytest.raises(TypeError, match="has no matching argument"):

        @app.get("/items/{item_id}")
        async def item() -> dict[str, Any]:
            return {}


def test_unsupported_annotation_fails_at_registration() -> None:
    app = Rayo()
    with pytest.raises(TypeError, match="not supported yet"):

        @app.get("/items/{item_id}")
        async def item(item_id: bytes) -> dict[str, Any]:
            return {}


def test_conflicting_routes_fail_at_startup() -> None:
    app = Rayo()

    @app.get("/users/{user_id}")
    async def by_id(user_id: int) -> None:
        return None

    @app.get("/users/{name}")
    async def by_name(name: str) -> None:
        return None

    with pytest.raises(ValueError, match="cannot register route"):
        app._start(port=0)
