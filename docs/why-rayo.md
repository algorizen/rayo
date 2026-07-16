# Why Rayo — the problems nobody has solved

Rayo is not a faster version of what already exists. It exists because six real problems in Python web development are unsolved today — not solved badly, but *unowned*. This page states each problem, the evidence it's real, Rayo's answer, and where it lands in the [roadmap](../ROADMAP.md).

## 1. The blocking-call footgun

**The problem.** One accidental synchronous call inside an `async def` handler stalls the entire event loop. Production case studies document p50 latency jumping 12ms → 250ms from a single sync DB call — and horizontal scaling doesn't help, it just spreads the pain. This has been the #1 recurring async-Python postmortem for a decade. No framework detects it; they all just document "be careful." The incumbents structurally cannot fix it without breaking `async def` semantics.

**Rayo's answer.** The Rust core watches handler execution. In dev mode, a blocking call is detected and **named with its file and line**. In prod mode it becomes a structured metric per route. Sync (`def`) handlers are always executed off-loop by construction, so the classic misconfiguration cannot occur at all. On free-threaded Python the trap disappears entirely: sync handlers run thread-per-request with real parallelism.

**Plan:** scheduler in Milestone 1, guard in Milestone 2 → [design](design/dispatch-scheduler.md).

## 2. Durable background work

**The problem.** The space between fire-and-forget `BackgroundTasks` (documented losing 1,706 records in a single load test; tasks silently discarded on errors) and a full Celery deployment is owned by **no Python framework**. Every AI application hits it: an agent run that must finish and persist whether or not the request survives.

**Rayo's answer.** `@task(retries=3, persist=True)` — a Rust-side job queue with lease-based execution, jittered retries, dead-letter queues, scheduling, and dependency injection. In-memory by default; SQLite/Postgres/Redis persistence one line away. Honest semantics, stated plainly: at-least-once with persistence, at-most-once without. The proof is a kill-test in CI: enqueue under load, SIGKILL the process, restart, zero loss.

**Plan:** Milestone 3 → [design](design/durable-jobs.md).

## 3. Free-threaded-native serving

**The problem.** Free-threaded Python is officially supported (3.14t), and one-event-loop-per-thread in a single process is now documented CPython architecture with measured near-linear multi-core scaling. Yet every framework with traction is architecturally anchored to "fork N workers and shard everything." Consequence: nobody offers a connection pool shared across all cores, process-accurate rate limits, in-process pub/sub, or a shared cache — without IPC and without running Redis for a single node.

**Rayo's answer.** Two worker topologies from one codebase: threads + loop-per-core on free-threaded builds, process workers on GIL builds — same user code. On top of that, shared-state primitives (`Pool`, `Cache`, `RateLimiter`, `Channel`) that are genuinely one-per-process on free-threaded Python and degrade honestly (same API, documented per-worker scope) on GIL builds. Fork-based frameworks cannot copy this without a rewrite. This is the structural moat.

**Plan:** topologies in Milestone 1, primitives in Milestone 3, launch timed to Python 3.15 GA → [design](design/shared-state.md), [ADR-0002](adr/0002-free-threaded-first.md).

## 4. Agent-era transport semantics

**The problem.** The characteristic modern request is a streaming, long-lived, must-not-be-lost unit of work — and today's primitives are wrong for it. A client disconnect force-cancels your LLM generator mid-run (with dependency teardown mid-flight). WebSocket auth dependencies have been broken in the leading framework since 2023. SSE requires third-party packages. Streaming backpressure is unbounded buffering in practice, even in the best Rust servers.

**Rayo's answer.** Detached streams: `on_disconnect=Detach` means the agent run completes and persists, and the client resumes via `Last-Event-ID` — an agent run survives a browser refresh. WebSocket auth works identically to HTTP routes. SSE is built in. Every Python-produced byte stream flows through bounded channels whose sends await capacity — TCP flow control becomes visible backpressure in Python. MCP endpoints mount next to REST routes: one app, one auth story, one deploy.

**Plan:** SSE/WebSockets in Milestone 2; detached/resumable streams and MCP in Milestone 3 → [design](design/streaming.md).

## 5. Cold start at scale

**The problem.** Large apps on the incumbent stack take 20–40 seconds to start (response models deep-copied per route, schemas rebuilt from scratch on every boot). Serverless and scale-to-zero deployments pay this tax constantly.

**Rayo's answer.** Type hints compile to a flat, serializable IR; Rust builds validators in one linear pass; compiled schemas are cached by content hash across restarts. Startup cost is O(changed schemas), not O(app size). Target, gated by a CI benchmark suite: sub-100ms with 1,000 routes.

**Plan:** Milestone 1, cache in Milestone 2 → [ADR-0006](adr/0006-schema-ir-compilation.md).

## 6. The zero-Python response path

**The problem.** Even Rust-powered validation libraries eagerly materialize a full Python object graph for every request and response — and measurements attribute ~95% of JSON handling cost to Python object creation, not parsing. The fastest validation core in the ecosystem still pays it.

**Rayo's answer.** One-pass parse + validate straight into Rust-native slotted models (Python field objects created only on first access), and responses serialized directly from Rust state to JSON bytes — **zero Python objects created for output**, enforced by an allocation hook in the test suite. A 422 never touches Python at all.

**Plan:** Milestone 1 → [ADR-0005](adr/0005-eager-slotted-models.md).

---

## How the roadmap maps to the problems

| Phase | What ships | Problems attacked |
|---|---|---|
| M0–M1 (foundation + spine) | Server, scheduler, schema compiler, both topologies | #1, #5, #6 |
| M2 (first-hour parity) | DI, OpenAPI, docs, compatibility scoreboard, blocking guard | #1 complete — and the DX parity without which nothing else converts |
| M3 (the moats) | Durable jobs, detached streams, shared state, MCP | #2, #3, #4 |
| M4 (launch, Python 3.15 GA) | Three demos, each proving an unsolved problem is solved | — |

The launch demos are the thesis made visible: (1) one process saturating 16 cores through a single shared pool (#3), (2) an agent stream surviving a browser refresh and resuming (#2 + #4), (3) the framework catching a blocking bug and printing its file:line (#1).

## An honest caveat

Problems 1–4 are durable moats: fixing them upstream requires breaking changes the incumbents cannot make. Problems 5–6 are **windows** — the incumbent's funded team is already working on startup pathologies. We market cold start and raw-path efficiency early and hard, but the brand is built on 1–4.
