# Rayo Architecture

This document describes the system architecture. The *why* behind each decision lives in [docs/adr/](docs/adr/); subsystem details live in [docs/design/](docs/design/).

## 1. The one rule

**Python is only for business logic.** Everything else — accepting connections, TLS, HTTP parsing, routing, validation, serialization, backpressure, jobs, shared state — happens in Rust. The request crosses the Rust↔Python boundary exactly once, and only if a user handler must run.

A PyO3 boundary crossing costs ~30 ns; data *conversion* costs far more (it is per-object, not per-call). The architecture exists to minimize both: cross once, hand over lazy Rust-backed objects, never make chatty per-field calls.

**Boundary budget: the framework adds < 1 µs of overhead per request** beyond the user's handler work. Changes that regress this budget need an ADR.

## 2. Process topology

```
┌──────────────────────── one process ─────────────────────────┐
│ Tokio multi-thread runtime (Rust)                             │
│   listener → TLS → Hyper → radix router → parse + validate    │
│   (404/405/422 answered without ever touching Python)         │
│        │ validated request as Rust slotted model              │
│        ▼ single Python::attach per request                    │
│ Free-threaded build: N handler threads / one asyncio loop     │
│   per core, ONE interpreter, shared state                     │
│ GIL build: same code, process-based workers                   │
│   custom coroutine scheduler (PyIter_Send stepping,           │
│   freelisted awaitables — no asyncio.Task per request)        │
│        │ handler returns Rust-backed response                 │
│        ▼ oneshot channel                                      │
│ Rust-direct serialization → Hyper response                    │
│ Rust sidecars: durable job queue · shared Pool/Cache/         │
│   RateLimiter/Channel · SSE/WS with bounded-channel           │
│   backpressure                                                │
└───────────────────────────────────────────────────────────────┘
```

### Two run modes, one codebase
- **Free-threaded Python (3.14t+):** workers are threads in a single interpreter. Sync handlers run thread-per-request; async handlers run on one event loop per core (the officially documented CPython 3.14 asyncio architecture). State primitives are genuinely shared.
- **GIL Python (3.10–3.14):** workers are processes sharing the listening socket. Identical user code; state primitives degrade to per-worker with the same API.

The Rust core is identical in both modes; only worker topology differs. At startup Rayo detects any imported C extension that would silently re-enable the GIL on a t-build and reports it by name.

## 3. Layers

### 3.1 Server (crate: `rayo-server`)
Tokio + Hyper 1.x, embedded — Rayo *is* the server; there is no uvicorn/gunicorn in the stack. HTTP/1.1 + HTTP/2, rustls TLS. Operational features are first-class: worker lifecycle, max-RSS recycling, graceful reload, structured logging, Prometheus metrics.

### 3.2 Routing (crate: `rayo-router`)
Radix-tree matching (`matchit`-class). Routes are registered from Python decorators at import time and compiled into the Rust router at startup. Route decorators are standalone (not bound to the app object), eliminating the circular-import problem by construction.

### 3.3 Schema & validation (crate: `rayo-schema`)
- At import time, Python-side introspection reads handler type hints (native models, Pydantic models, dataclasses, TypedDicts) and emits a **flat, serializable IR** (postorder node list, integer child refs).
- Rust compiles the IR into a validator/serializer tree in a single linear pass. Compiled schemas are **content-hash cached across restarts** — large apps cold-start in milliseconds.
- Request path: jiter-style pull-parsing performs **one-pass parse + validate** straight into Rust-native slotted model objects. No intermediate dicts, no Python objects for fields never touched (materialization is memoized on first access; buffers are Arc-owned by the model, so no use-after-parse hazards).
- Response path: handlers return Rayo models (or pydantic/dataclass values via the bridge); serialization is **Rust-direct to JSON bytes — zero Python objects are created for output**.

### 3.4 Dispatch (crate: `rayo-dispatch`)
The Rust↔Python bridge. A frozen `#[pyclass]` scheduler steps handler coroutines by calling `PyIter_Send` directly (no `asyncio.Task` per request), with freelisted awaitable objects and `call_soon_threadsafe` used only for cross-thread wakeups. This is the design Granian proved (its measurements: 38–55% faster than generic pyo3-asyncio glue) — see ADR-0004. `pyo3-async-runtimes` is used only on cold paths (lifespan events).

Cancellation is explicit and bidirectional: client disconnects propagate to Python as cancellation *unless* the route opts into detached mode (see 3.7).

### 3.5 Dependency injection (Python: `rayo/di.py`, resolution in Rust)
The dependency graph is resolved and validated **at startup**, not per request. Scopes: `app`, `request` (and `job` inside tasks). Misconfiguration produces a startup error naming the full chain. Request-time resolution is a precomputed plan executed in Rust.

### 3.6 The blocking-call guard
The dispatcher measures continuous on-loop execution per handler step. In dev mode, a step exceeding the threshold produces a warning with the offending file:line (via a sampling profiler hook). In prod mode, handlers declared `def` are automatically executed off-loop in a sized, first-class-configurable thread pool; `async def` handlers that block trigger structured warnings with rate limiting. Async Python's #1 production failure mode becomes a diagnosed condition instead of a mystery.

### 3.7 Streaming, SSE, WebSockets (crate: `rayo-stream`)
- All Python-produced byte streams flow through **bounded channels whose `send` awaits capacity** — hyper/TCP flow control becomes visible backpressure in Python. No unbounded buffering anywhere.
- SSE is built in (no third-party package), with correct disconnect semantics in *both* directions: default = cancel on disconnect; `detached=True` = the generator runs to completion, output is persisted, and clients resume via `Last-Event-ID`.
- WebSockets: auth dependencies work identically to HTTP routes; ping/pong handled in Rust; built-in connection manager and rooms backed by `rayo.Channel`.
- MCP endpoints mount alongside REST routes with the same decorator surface.

### 3.8 Durable jobs (crate: `rayo-jobs`)
A Rust-side queue with retries, scheduled execution, and delivery guarantees. Backends: in-memory (default), SQLite, Postgres, Redis. Jobs accept dependency injection, outlive the request lifecycle, and (with a persistent backend) survive restarts. This is a framework primitive, not an integration.

### 3.9 Shared-state primitives (crate: `rayo-state`)
`rayo.Pool`, `rayo.Cache`, `rayo.RateLimiter`, `rayo.Channel`. On free-threaded builds these are one shared instance across all cores (no IPC, no external services for single-node). On GIL builds, the same API is per-worker. All interior state is `Sync` Rust (`Mutex`/atomics/lock-free maps) — thread-safe by construction.

### 3.10 Interop
- **ASGI adapter:** mount existing ASGI apps under Rayo, and run pure-ASGI middleware (auth, tracing, CORS) at a documented ~20% streaming-path cost, opt-in per mount. The native path stays default. A protocol is not a moat; the ecosystem is.
- **Schema bridge:** Pydantic models and dataclasses are accepted anywhere Rayo models are — migration from existing type-hint-based frameworks is designed, tested (public compatibility scoreboard), and cheap.

## 4. Packaging & distribution

- PyO3 ≥ 0.29 with `gil_used = false`; frozen pyclasses; all shared state `Sync`.
- maturin builds; **per-version wheels** cp310–cp314 plus cp314t (no abi3 initially — it forfeits version-specific fast paths); adopt `abi3.abi3t` dual-tagging when Python 3.15 tooling matures (PEP 803).
- Platform matrix: manylinux2014 + musllinux, macOS universal2/arm64, Windows x64. Users never need Rust installed; sdist is a fallback, not the path.

## 5. Crate/package layout (planned)

```
rayo/
├── crates/
│   ├── rayo-core/       # glue: config, lifecycle, worker topology
│   ├── rayo-server/     # tokio + hyper, TLS, ops features
│   ├── rayo-router/     # radix routing
│   ├── rayo-schema/     # IR compiler, validators, serializers
│   ├── rayo-dispatch/   # scheduler, boundary, blocking guard
│   ├── rayo-stream/     # SSE/WS/streaming, backpressure
│   ├── rayo-jobs/       # durable queue + backends
│   └── rayo-state/      # Pool/Cache/RateLimiter/Channel
├── python/rayo/         # decorator surface, DI, introspection→IR,
│                         # ASGI adapter, pydantic bridge, testing utils
├── benchmarks/           # reproducible, Dockerized (see docs/benchmarks-policy.md)
├── conformance/          # migration-compatibility suite + scoreboard
└── docs/
```

## 6. Invariants (enforced in review)

1. One `Python::attach` per request on the hot path.
2. No unbounded channel carries Python-produced data.
3. No Python object creation on the response serialization path.
4. Rust core state is `Sync`; nothing relies on the GIL for correctness.
5. Startup fails loudly and specifically (schema errors, DI errors, GIL re-enablement) — never lazily at request time.
6. The boundary budget: < 1 µs framework overhead per request.
