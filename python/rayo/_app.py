"""The Rayo application object and route registration.

Import-time work here is introspection only (hard invariant 6): signatures
and type hints become route specs; everything heavier happens in Rust at
startup. Registration errors are raised immediately, at the decorator, with
the file and line of the handler that caused them (invariant 5).
"""

from __future__ import annotations

import inspect
import os
import re
import sys
from collections.abc import Callable
from typing import Any, get_type_hints

from rayo import _core

Handler = Callable[..., Any]
_RouteSpec = tuple[str, str, Handler, bool, list[tuple[str, int]]]

_PATH_PARAMETER = re.compile(r"\{(\w+)\}")

_PARAM_KIND_BY_ANNOTATION: dict[type, int] = {str: 0, int: 1, float: 2, bool: 3}


class Rayo:
    """The Rayo application object.

    v0 surface: route decorators for handlers whose only inputs are typed
    path parameters, and :meth:`run`. Query parameters, bodies, dependency
    injection, and typed response models arrive through Milestones 1-2
    (see PLAN.md).
    """

    def __init__(self, *, title: str = "Rayo app") -> None:
        self.title = title
        self._routes: list[_RouteSpec] = []

    def get(self, path: str) -> Callable[[Handler], Handler]:
        return self._register("GET", path)

    def post(self, path: str) -> Callable[[Handler], Handler]:
        return self._register("POST", path)

    def put(self, path: str) -> Callable[[Handler], Handler]:
        return self._register("PUT", path)

    def patch(self, path: str) -> Callable[[Handler], Handler]:
        return self._register("PATCH", path)

    def delete(self, path: str) -> Callable[[Handler], Handler]:
        return self._register("DELETE", path)

    def _register(self, method: str, path: str) -> Callable[[Handler], Handler]:
        def add_route(handler: Handler) -> Handler:
            self._routes.append(_build_route_spec(method, path, handler))
            return handler

        return add_route

    def run(
        self,
        *,
        host: str = "127.0.0.1",
        port: int = 8000,
        loop_threads: int | None = None,
    ) -> None:
        """Serve until Ctrl+C, then shut down gracefully.

        ``loop_threads`` is the number of Python event-loop threads handling
        requests. Default: one per CPU core on free-threaded builds (they run
        handlers truly in parallel), one total on GIL builds (where more loops
        add switching overhead, not parallelism).
        """
        resolved_loop_threads = _resolve_loop_threads(loop_threads)
        server = self._start(host=host, port=port, loop_threads=resolved_loop_threads)
        topology = (
            f"{resolved_loop_threads} event-loop thread"
            f"{'s' if resolved_loop_threads != 1 else ''}"
            f"{' — free-threaded parallelism' if _is_free_threaded() else ''}"
        )
        print(f"Rayo serving on http://{host}:{server.port} ({topology}; Ctrl+C to stop)")
        try:
            server.wait()
        except KeyboardInterrupt:
            pass
        finally:
            server.shutdown()

    def _start(
        self,
        *,
        host: str = "127.0.0.1",
        port: int = 0,
        loop_threads: int | None = None,
    ) -> _core.Server:
        """Start serving in the background and return the server handle.

        Internal for now (tests and tooling); the public embedding API is
        designed in M1 alongside the worker topologies.
        """
        return _core.start_server(host, port, self._routes, _resolve_loop_threads(loop_threads))


def _is_free_threaded() -> bool:
    gil_check = getattr(sys, "_is_gil_enabled", None)
    return gil_check is not None and not gil_check()


def _resolve_loop_threads(loop_threads: int | None) -> int:
    if loop_threads is not None:
        if loop_threads < 1:
            raise ValueError(
                f"loop_threads must be at least 1, got {loop_threads}. Leave it unset "
                f"to let Rayo pick: one per core on free-threaded builds, one on GIL builds."
            )
        return loop_threads
    if _is_free_threaded():
        return os.cpu_count() or 1
    return 1


def _handler_location(handler: Handler) -> str:
    code = getattr(handler, "__code__", None)
    if code is None:
        return repr(handler)
    return f"{handler.__qualname__} ({code.co_filename}:{code.co_firstlineno})"


def _build_route_spec(method: str, path: str, handler: Handler) -> _RouteSpec:
    path_parameter_names = _PATH_PARAMETER.findall(path)
    signature = inspect.signature(handler)
    type_hints = get_type_hints(handler)

    for argument_name in signature.parameters:
        if argument_name not in path_parameter_names:
            raise TypeError(
                f"{method} {path}: handler argument '{argument_name}' does not match any "
                f"path parameter on {_handler_location(handler)}. v0 handlers can only "
                f"take path parameters; query and body parameters arrive with the "
                f"schema engine (see ROADMAP.md)."
            )

    parameter_specs: list[tuple[str, int]] = []
    for parameter_name in path_parameter_names:
        if parameter_name not in signature.parameters:
            raise TypeError(
                f"{method} {path}: path parameter '{{{parameter_name}}}' has no matching "
                f"argument on {_handler_location(handler)}. Add '{parameter_name}: str' "
                f"(or int, float, bool) to the handler signature."
            )
        annotation = type_hints.get(parameter_name, str)
        parameter_kind = _PARAM_KIND_BY_ANNOTATION.get(annotation)
        if parameter_kind is None:
            supported = ", ".join(
                supported_type.__name__ for supported_type in _PARAM_KIND_BY_ANNOTATION
            )
            raise TypeError(
                f"{method} {path}: path parameter '{parameter_name}' on "
                f"{_handler_location(handler)} is annotated {annotation!r}, which is not "
                f"supported yet. Supported path parameter types: {supported}."
            )
        parameter_specs.append((parameter_name, parameter_kind))

    is_async = inspect.iscoroutinefunction(handler)
    return (method, path, handler, is_async, parameter_specs)
