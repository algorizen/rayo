# Design: Dispatch, Scheduler, and the Blocking Guard

**Crate:** `rayo-dispatch` · **Related:** ADR-0001, ADR-0004 · **Status:** Draft

## Goals

Run user handlers (sync and async) from the Tokio runtime with <1 µs framework overhead, correct cancellation, contextvars propagation, and both worker topologies (ADR-0002). Detect and defuse blocking calls — the ecosystem's #1 production failure.

## Non-goals

Being a general asyncio↔Tokio bridge (that's pyo3-async-runtimes; we use it for lifespan only). Reimplementing the event loop (ADR-0007).

## Design

### Request dispatch (async handlers)

1. Hyper task completes routing + validation in Rust; the request exists as a Rust slotted model.
2. `spawn_blocking` → **single `Python::attach`**: build the handler call (vectorcall with Rust tuples), obtain the coroutine, hand it to the per-thread scheduler. GIL builds: this is the one GIL acquisition; FT builds: attach is non-serializing.
3. The **scheduler** (frozen `#[pyclass]`, freelisted) steps the coroutine with `PyIter_Send` directly — no `asyncio.Task` per request. Yielded awaitables get a done-callback waker; cross-thread wakeups use `call_soon_threadsafe`; same-loop wakeups use `call_soon`.
4. Handler result (Rust-backed response) returns over a `tokio::oneshot` to the Hyper task; serialization is Rust-direct (ADR-0005).

Sync handlers: GIL builds → sized thread pool (first-class config: size, queue depth, timeout); FT builds → thread-per-request (measured 3.7x territory), same pool config semantics.

### Cancellation

Client disconnect → cancellation token → scheduler throws `CancelledError` into the coroutine at its next suspension point. Dependencies with cleanup (context managers) unwind normally. Routes/streams marked `detached=True` swap the token for a completion-tracked handle (see streaming design). Cancellation is **tested behavior**, including: disconnect during validation (never enters Python), during handler await, during response streaming.

**Sync handlers cannot be cancelled mid-run.** A `def` handler runs to completion on its blocking-capable thread regardless of the client's fate — there is no suspension point to deliver `CancelledError` to. If the client is gone by the time it finishes, the response is discarded (the response channel's receiver has dropped) and the thread returns to its pool. This is the same semantics `run_in_executor` gives asyncio, and it is documented user-facing behavior, not an accident: handlers that need cooperative cancellation must be `async`.

### contextvars

The scheduler captures the context at dispatch and enters it for every step — equivalent to `asyncio.Task` semantics. Request-scoped context (trace IDs, DI request scope) rides contextvars; this is load-bearing for observability integrations and covered by conformance tests.

### The blocking guard

- A watchdog thread samples per-scheduler "currently stepping since" timestamps.
- **Dev mode:** a step exceeding `blocking_threshold` (default 50 ms) triggers a stack capture of the offending thread → warning with file:line, rate-limited per route.
- **Prod mode:** same detection feeds a structured warning + metric (`rayo_blocked_steps_total{route=…}`). No automatic punishment of async handlers (magic offloading mid-coroutine is unsound); the fix loop is: metric → dev-mode repro → file:line.
- Sync (`def`) handlers are *always* off-loop by construction, so the classic sync-work-on-the-event-loop misconfiguration cannot occur.

### FT topology

One scheduler per worker thread; one asyncio loop per core for async handlers (documented CPython 3.14 pattern). Long Rust-only work always runs detached (`Python::detach`) so stop-the-world GC never waits on us.

## Open questions

1. Freelist sizing/eviction policy for awaitables under bursty load.
2. Should `blocking_threshold` auto-calibrate from observed p99 step time?
3. Sub-interpreter compatibility — out of scope for v1, but avoid painting it out.

## Test strategy

Boundary integration tests from Python (both builds); cancellation matrix; contextvars conformance vs `asyncio.Task` semantics; a deliberate-blocking test app asserting the guard's file:line output; dispatch micro-benchmarks gating the <1 µs budget in CI.
