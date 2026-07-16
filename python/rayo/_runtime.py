"""Internal bridge between the Rust dispatcher and the asyncio event loop.

`spawn_handler` runs on the event-loop thread (the Rust side schedules it via
``call_soon_threadsafe``). Whatever happens — including the handler failing
before its first await — `complete` is always called, so no request can hang.
"""

import asyncio
from collections.abc import Callable, Coroutine
from typing import Any

_HandlerCompletion = Callable[["asyncio.Future[object]"], None]


def spawn_handler(
    event_loop: asyncio.AbstractEventLoop,
    handler: Callable[..., Coroutine[Any, Any, object]],
    handler_kwargs: dict[str, Any],
    complete: _HandlerCompletion,
) -> None:
    try:
        handler_coroutine = handler(**handler_kwargs)
    except BaseException as handler_error:
        failed_call = event_loop.create_future()
        failed_call.set_exception(handler_error)
        complete(failed_call)
        return
    handler_task = event_loop.create_task(handler_coroutine)
    handler_task.add_done_callback(complete)
