# Rayo Web

**Rayo** (Spanish: *lightning bolt*) — **the web framework for free-threaded Python.**

Rayo is a Python web framework with a Rust core, designed from the first commit for the post-GIL era: one process, every core, shared state — and work that survives failure.

> **Status: pre-alpha, docs-first.** We are building in the open, design before code. Read the [plan](PLAN.md), the [architecture](ARCHITECTURE.md), and the [roadmap](ROADMAP.md). Discussions and RFCs are welcome now — this is the best moment to shape the framework.

## Why Rayo exists

The last generational insight in Python web frameworks was *the type hint is the API* — one annotation giving you validation, serialization, docs, and editor support. Rayo is built on the next one:

> **The process is the cluster, and work is durable.**

Three shifts converged in 2025–2026 and no existing framework was designed for them:

1. **Free-threaded Python is real** (PEP 703/779). One Python process can finally use every core with shared memory. Every current framework still forks N workers and shards everything.
2. **Rust↔Python fusion matured** (PyO3, maturin, pydantic-core). The entire request lifecycle can live in Rust; Python is reserved for what humans write — business logic.
3. **The workload changed.** LLM serving and agents made the characteristic request a streaming, long-lived, must-not-be-lost unit of work — the exact place today's frameworks are weakest.

## What it will look like

```python
from rayo import Rayo, get, task
from rayo.state import Pool

app = Rayo()
db = Pool("postgresql://...")   # ONE pool, shared by every core in the process

@get("/users/{user_id}")
async def read_user(user_id: int) -> User:      # type hints -> validation, docs, editor support
    return await db.fetch_user(user_id)

@task(retries=3, persist=True)
async def send_welcome_email(user_id: int):     # durable: survives errors, restarts, deploys
    ...
```

- Write `def` or `async def` — Rayo runs it correctly on every Python. A blocking call inside an async handler is **detected and named with its source line** instead of silently stalling your service.
- Validation, serialization, routing, and OpenAPI happen in Rust. A 422 never touches Python. Cold starts are measured in milliseconds, not tens of seconds.
- Streams can be **detached**: an agent run finishes and persists even if the client disconnects, and resumes via `Last-Event-ID`.
- On free-threaded Python (3.14t+): one process saturates all cores with a shared connection pool. On standard Python: the same code runs on proven multi-worker topology. One codebase, no ceremony.

## What Rayo is not

- **Not a benchmark project.** We publish reproducible, same-core-count benchmarks — including ones we lose ([benchmark policy](docs/benchmarks-policy.md)).
- **Not an ORM, template engine, or DB driver.** We integrate SQLAlchemy, asyncpg, and Jinja2 rather than replacing them.
- **Not a clone of any existing framework.** Compatibility is a bridge (pydantic-model input, ASGI middleware adapter, a public migration-compatibility scoreboard) — not the identity.

## Project documents

| Document | Purpose |
|---|---|
| [docs/why-rayo.md](docs/why-rayo.md) | The six unsolved problems Rayo exists to solve |
| [PLAN.md](PLAN.md) | Development plan: workstreams, milestones, exit criteria |
| [ARCHITECTURE.md](ARCHITECTURE.md) | System architecture and the request lifecycle |
| [ROADMAP.md](ROADMAP.md) | Public phased roadmap |
| [docs/adr/](docs/adr/) | Architecture decision records — why each big choice was made |
| [docs/design/](docs/design/) | Subsystem design docs (scheduler, schema compiler, durable jobs, …) |
| [docs/rfcs/](docs/rfcs/) | RFC process — propose changes to Rayo's design |
| [CONTRIBUTING.md](CONTRIBUTING.md) | How to contribute (most contributions need no Rust) |
| [GOVERNANCE.md](GOVERNANCE.md) | How decisions are made, how maintainers are added |
| [docs/code-standards.md](docs/code-standards.md) | Code standards for Rust and Python |
| [SECURITY.md](SECURITY.md) | Security policy and reporting |

## Contributing

Rayo is MIT-licensed and community-governed from day one. The Rust core is a minority of the surface — most features, docs, and integrations are pure Python. Start with [CONTRIBUTING.md](CONTRIBUTING.md) and the issues labeled `good-first-issue`.

## License

MIT
