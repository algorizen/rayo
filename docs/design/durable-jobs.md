# Design: Durable Jobs

**Crate:** `rayo-jobs` · **Related:** ADR-0002, ADR-0009 · **Status:** Draft

## Goals

The space between fire-and-forget `BackgroundTasks` (documented losing work under load) and a Celery deployment — as a framework primitive. Jobs that outlive the request, retry with policy, run on schedule, accept DI, and (persisted) survive restarts. Honest, simple delivery semantics.

## Non-goals

Distributed workflow orchestration (Temporal's job), exactly-once delivery (a lie we won't tell), cross-service queues (that's what real brokers are for — we integrate, ADR-0009).

## Design

### API

```python
@task(retries=3, backoff=Exponential(base=2), timeout=30, persist=True)
async def send_welcome_email(user_id: int, mailer: Mailer = dep(get_mailer)):
    ...

await send_welcome_email.enqueue(user.id)                    # from a handler
await send_welcome_email.schedule(user.id, at=tomorrow_9am)  # scheduled
```

`@task` functions are also plain callables (direct `await` works in tests). DI resolves in `job` scope: request-scoped deps are unavailable by construction (compile-time error at startup, not a runtime surprise) — jobs must not capture request lifetimes.

### Engine (Rust)

A Tokio-side queue with: per-queue concurrency limits, lease-based execution (a job is *leased* to a worker, re-queued if the lease lapses), retry policy with jittered backoff, dead-letter queue after exhaustion, scheduled/cron entries. Job payloads are the validated arguments, serialized via `rayo-schema` (same IR machinery — jobs get validation for free).

### Backends

| Backend | Survives | Use |
|---|---|---|
| `memory` (default) | nothing — but *does* run enqueued jobs to completion during graceful shutdown | dev, tests, best-effort work |
| `sqlite` | restarts (single node) | the "no infrastructure" production default |
| `postgres` | restarts, multi-process | teams already running Postgres (SKIP LOCKED leasing) |
| `redis` | restarts, multi-node | high-throughput / existing Redis |

Delivery semantics, stated plainly in docs: **at-least-once with persistent backends; at-most-once with `memory`**. Handlers should be idempotent; the docs teach the idempotency-key pattern with first-class support (`enqueue(…, idempotency_key=…)` dedupes within a window).

### Topology

FT builds: the queue and its workers live in-process, sharing state and pools — a single deployable unit. GIL builds: queue in each worker process by default; persistent backends make the queue shared across processes automatically (the backend is the coordination point). Same API throughout.

### Detached streams

The streaming design reuses this engine: a detached stream is a job whose output is an append-only event log with `Last-Event-ID` resume.

## Open questions

1. Job result storage/retrieval API (`await handle.result()`) — TTL policy.
2. Priorities: per-queue only, or per-job within a queue?
3. Observability: expose queue depth/age as Prometheus metrics from day one (leaning yes).

## Test strategy

Kill-the-process tests per persistent backend (enqueue → SIGKILL → restart → assert execution); lease-expiry races; retry/backoff timing; DI-scope compile-time errors; load test replicating the documented ecosystem failure mode of in-process background tasks dropping work under load (high-load enqueue, zero loss with sqlite backend).
