# ADR-0004: Custom coroutine scheduler, not generic async glue

**Status:** Accepted — 2026-07-16

## Context

Running Python async handlers from a Tokio runtime has three known approaches. Generic glue (`pyo3-async-runtimes`): correct, maintained, but allocates an `asyncio.Task` per request and pays `ensure_future` + done-callback machinery — the Rust-core frameworks that ship it show the cost in their numbers. Custom scheduler (Granian): a frozen pyclass steps handler coroutines by calling `PyIter_Send` directly via FFI, uses freelisted awaitable objects, and reserves `call_soon_threadsafe` for cross-thread wakeups — Granian's own source documents **38–55% faster dispatch** than the generic glue. Thread-per-request without a loop (free-threaded only): measured 3.7x for sync workloads on 3.14t.

## Decision

Port Granian's scheduler design (MIT-licensed prior art, credited): direct coroutine stepping, freelisted awaitables, oneshot-channel responses, single attach per request. Sync handlers on FT builds run thread-per-request. `pyo3-async-runtimes` remains for cold paths only (lifespan events).

Cancellation is a first-class scheduler concern: client disconnects propagate as coroutine cancellation by default, and the detached-stream mode (jobs/streams design doc) opts out deliberately.

## Consequences

- We own subtle FFI code (`unsafe` confined here per code standards, `// SAFETY:` required) and its maintenance.
- Dispatch overhead stays within the <1 µs boundary budget at realistic concurrency.
- contextvars propagation and cancellation semantics must be explicitly tested — they don't come free as they would with `asyncio.Task`.
