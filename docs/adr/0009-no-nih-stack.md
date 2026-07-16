# ADR-0009: No ORM, no template engine, no DB drivers

**Status:** Accepted — 2026-07-16

## Context

The graveyard is explicit on this. One framework built its own schema engine, async template engine, and HTTP client — one person could not maintain the surface, and the project died announcing a rewrite. Another built an in-house ORM, templating, and i18n over a decade of excellent solo engineering — and got no ecosystem leverage. Meanwhile the fastest-growing newcomer of 2025 won precisely by attaching to an existing ecosystem, and pydantic-core won by being a *layer* everyone else could adopt.

Protocol drivers are decade-long correctness projects. The free-threaded story doesn't need them: asyncpg already ships FT wheels; the Rust-native psqlpy currently ships none (verified July 2026).

## Decision

Rayo builds **framework infrastructure**, not stack replacements:

- **Never:** ORM, template engine, database/wire-protocol drivers, HTTP client.
- **Integrate and document:** SQLAlchemy, asyncpg, Jinja2, httpx — with a maintained **FT-safe dependency allowlist** (asyncpg over psycopg, jiter path over orjson) that turns ecosystem landmines into framework guardrails.
- **Build (because they're framework-shaped and exploit our architecture):** shared connection *pooling* (wrapping real drivers), cache, rate limiter, pub/sub, durable job queue.
- Where the ecosystem blocks free-threading, prefer **upstream contributions** (e.g., FT wheels for psqlpy) over forks or replacements.

## Consequences

- Rayo's surface stays maintainable by a small team; ecosystem knowledge transfers in.
- We accept dependency risk (an upstream regression is our user's outage) — mitigated by the allowlist's version pinning and CI against pinned + latest.
