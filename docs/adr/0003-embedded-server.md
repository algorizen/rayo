# ADR-0003: Embedded server; native protocol inside, ASGI adapter outside

**Status:** Accepted — 2026-07-16

## Context

Pure-Python frameworks need an external server (uvicorn/gunicorn) because they can't own sockets efficiently. Our Rust core can — Granian proved the embedded Tokio+Hyper model in production (2.5–10x uvicorn-class throughput, adopted for ops qualities: worker recycling, RSS limits, consistent tails).

On protocol: Granian's RSGI (typed native protocol) measures +23% over ASGI on echo/streaming paths — but got **zero external adoption** in four years, and mainstream ASGI frameworks declined to support it. Meanwhile "pure ASGI middleware" (auth, tracing, CORS) is a real, portable ecosystem. Frameworks that bypassed ASGI without an adapter cut themselves off from middleware, APM agents, and shared knowledge.

## Decision

Rayo **is** the server — Tokio + Hyper embedded, no uvicorn in the stack, with Granian-grade operational features treated as launch requirements, not afterthoughts. Internally, requests flow through native Rust-backed objects (thinner than any protocol: handlers receive validated typed models, not scopes). Externally, an **ASGI adapter** ships in Phase 1: mount ASGI apps, run pure-ASGI middleware, at a documented (~20% streaming-path) opt-in cost.

The native protocol is an implementation detail, never a pitch. We do not ask the ecosystem to adopt it.

## Consequences

- We own the full ops surface (lifecycle, reload, metrics) — significant scope, and the reason adoption decisions went Granian's way.
- ASGI compatibility bounds our incompatibility risk with the existing ecosystem.
- Anything Starlette-coupled (BaseHTTPMiddleware subclasses) won't run; documented honestly.
