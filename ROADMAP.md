# Rayo Roadmap

Public, honest, and updated as reality changes. Dates are targets, not promises. Discussion happens in GitHub Discussions; design changes go through [RFCs](docs/rfcs/).

## Phase 0 — Foundation (now → ~3 months)

**Goal: hello-world → CRUD works on both GIL and free-threaded Python, with the architecture proven.**

- [x] Research: ecosystem history, framework pain-point evidence, free-threading readiness, boundary engineering
- [x] Product plan, architecture, decision records
- [ ] Reserve `rayo` (+ `rayoweb` defensively) on PyPI; create `github.com/algorizen/rayo`; set up docs subdomain on algorizen.ai
- [x] Cargo workspace + maturin skeleton; CI matrix (Linux/mac/Windows × cp310–cp314 + cp314t)
- [ ] `rayo-server`: Tokio/Hyper embedded server, HTTP/1.1 + HTTP/2
- [ ] `rayo-router`: radix routing, standalone decorators
- [ ] `rayo-dispatch`: coroutine scheduler (Granian-style), single-attach boundary
- [ ] `rayo-schema` v0: type-hint introspection → flat IR → Rust validators; slotted models; Rust-direct response serialization
- [ ] Two run modes: process workers (GIL) and thread workers + loop-per-core (3.14t)
- [ ] Benchmark harness in-repo (Dockerized, same-core-count — see [policy](docs/benchmarks-policy.md))

## Phase 1 — First-hour parity (~3–6 months)

**Goal: the best first-hour developer experience in Python web — typed handlers, instant docs, editor magic — at parity with the best or better.**

- [ ] Dependency injection: startup-resolved graph, app/request scopes, precise errors
- [ ] OpenAPI 3.1 generation + Swagger/Scalar UI at `/docs`
- [ ] Middleware system + ASGI adapter (mount ASGI apps, run pure-ASGI middleware)
- [ ] SSE + WebSockets with working auth dependencies and bounded-channel backpressure
- [ ] Pydantic-model and dataclass schema input (the migration bridge)
- [ ] Public migration-compatibility suite + scoreboard
- [ ] Error-message polish pass (startup, validation, DI, 4xx bodies)
- [ ] Docs: tutorial layer + API reference skeleton; llms.txt
- [ ] Continuous PyPI releases — the repo is never ahead of the released package
- [ ] First outside contributors with merge rights on the Python layer

## Phase 2 — The new capabilities (~6–10 months)

**Goal: the things no Python framework has. Launch.**

- [ ] `rayo-jobs`: durable task queue (retries, scheduling, DI); in-memory + SQLite + Postgres + Redis backends
- [ ] Detached/resumable streams (`Last-Event-ID` resume; agent runs survive disconnects)
- [ ] Blocking-call guard: dev-mode file:line diagnosis, prod-mode auto-offload
- [ ] Shared-state primitives: `Pool`, `Cache`, `RateLimiter`, `Channel`
- [ ] MCP endpoint mounting with the decorator surface
- [ ] Ops: worker recycling, max-RSS limits, graceful reload, Prometheus metrics
- [ ] Docs: "structuring real applications" guide; agent-skill files
- [ ] **Public launch aligned with Python 3.15 GA (October 2026)** — abi3t lands; free-threading's first mainstream year
- [ ] Launch demos: (1) 16 cores, one process, one shared pool; (2) agent stream surviving a browser refresh; (3) the framework catching a blocking bug with file:line

## Phase 3 — The ecosystem grows (10+ months)

- [ ] Integration guides: SQLAlchemy, asyncpg, vLLM serving, auth recipes
- [ ] Template repos + Docker images (including 3.14t/3.15t)
- [ ] Serverless cold-start showcase
- [ ] Plugin registry; stable plugin API
- [ ] Upstream contributions where the ecosystem blocks free-threading (e.g., FT wheels for missing drivers)
- [ ] Evaluate a minimal Rust event loop — only if profiling justifies it (ADR-0007)

## Non-goals (see [PLAN.md](PLAN.md))

ORM, template engine, database drivers, benchmark-leaderboard marketing, forking existing frameworks, free-threading-only support.
