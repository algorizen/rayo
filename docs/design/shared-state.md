# Design: Shared-State Primitives

**Crate:** `rayo-state` · **Related:** ADR-0002, ADR-0009 · **Status:** Draft

## Goals

The free-threaded showcase: one process, N cores, genuinely shared infrastructure — no IPC, no single-node Redis. Same API on GIL builds (per-worker, documented). Thread-safe by construction (`Sync` Rust interiors; nothing trusts the GIL).

## The primitives

### `rayo.Pool`
Connection pooling *around* real drivers (never replacing them — ADR-0009). FT builds: one pool, all cores — checkout/checkin are lock-free fast paths; per-connection affinity to the checking-out loop where the driver requires it (asyncpg connections are loop-bound: the pool tracks home-loop and hands connections back to their loop, or opens per-loop sub-pools — open question #1). GIL builds: per-worker pool, same API. Metrics: size, in-use, wait time.

### `rayo.Cache`
In-process concurrent map (sharded, TTL, LRU bound, singleflight `get_or_compute` — concurrent misses for one key compute once). Values are Python objects: on FT builds sharing is direct (documented: cache immutable values); a `bytes`-mode stores serialized values for strict isolation. Not a distributed cache; docs say when to graduate to Redis.

### `rayo.RateLimiter`
Token bucket / sliding window in Rust atomics, keyed (per-IP, per-user, per-route). On FT builds limits are process-accurate; on GIL builds per-worker (docs show the ×workers math loudly — silent inaccuracy is a bug class we refuse).

### `rayo.Channel`
In-process pub/sub: named topics, bounded per-subscriber queues (lagging subscribers drop-oldest or disconnect by policy — never unbounded growth), feeding websocket rooms and SSE fan-out. FT: cross-core in-process. GIL: per-worker, with a documented Redis-backed upgrade path via the same API.

## Design rules

1. Every primitive is honest about topology: `.scope` property reports `"process"` or `"worker"`; startup logs it.
2. Bounded everything: caches have max sizes, channels have queue bounds, pools have limits — configuration is required thinking, defaults are conservative.
3. No global registry magic: primitives are constructed in app setup and reach handlers via DI (app scope).
4. Interior state is Rust `Sync` (sharded locks/atomics); Python-visible objects are frozen pyclasses.

## Open questions

1. Pool × loop-per-core: per-loop sub-pools vs home-loop handback (benchmark both; asyncpg is the forcing driver).
2. `Cache` mutability footgun on FT: warn-on-mutable-value heuristic, or docs-only?
3. `Channel` persistence hook — overlaps durable-jobs event logs; unify or keep separate?

## Test strategy

FT correctness under `pytest-run-parallel`-style concurrent access (the uvloop FT races are the cautionary precedent); loom-style model checking for the Rust interiors where practical; the `free-threaded` benchmark suite (cores vs throughput with one shared pool) as a regression gate; GIL-build API parity tests.
