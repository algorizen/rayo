# ADR-0007: No custom asyncio event loop in v1; loop is pluggable

**Status:** Accepted — 2026-07-16

## Context

"uvloop but Rust" exists: rloop (by Granian's author — self-described "definitely not suited for production," Unix-only) and rsloop (young, io_uring, claims 2x uvloop on callbacks; both ship cp314t wheels). A full asyncio-compatible loop is a multi-year compatibility project: SSL, subprocesses, signals, UDS, `sock_*` — exactly the features those projects still lack. Even Granian's author keeps rloop as a separate experiment rather than Granian's engine.

Decisively: in Rayo's architecture the event loop is demoted. Tokio owns all network I/O; the asyncio loop only steps handler coroutines and services their awaits (DB drivers, HTTP clients). The 2–4x uvloop-vs-stdlib gap that matters under uvicorn mostly evaporates when the loop isn't doing socket I/O.

## Decision

No custom event loop in v1. The loop is **pluggable** (`--loop uvloop|rloop|rsloop|stdlib|auto`), defaulting to uvloop on GIL builds (FT wheels since 0.22.1; note its FT races were fixed as late as Jan 2026 — pin accordingly) and stdlib on t-builds until alternatives prove FT-stable.

Revisit in Phase 3 **only if profiling shows the loop as a bottleneck**: by then we'll know the exact small subset of loop API our scheduler exercises, and a minimal purpose-built loop becomes a bounded project instead of an asyncio reimplementation.

## Consequences

- Zero loop-compatibility maintenance now; we inherit uvloop's maturity.
- A future minimal loop, if ever justified, has a written scope test: "implements only what `rayo-dispatch` calls."
