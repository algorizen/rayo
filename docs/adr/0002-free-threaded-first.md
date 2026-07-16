# ADR-0002: Free-threaded-first, GIL-compatible

**Status:** Accepted — 2026-07-16

## Context

PEP 779 is Final: free-threaded Python is officially supported as of 3.14t (5–11% single-thread overhead; JIT-on-FT expected ~3.16). One-loop-per-thread multi-core serving in a single process is now documented CPython architecture with measured near-linear scaling. The stable ABI for t-builds (abi3t, PEP 803) lands in Python 3.15 (Oct 2026). The default flip is realistically 2028–2031 (core-dev estimate). Ecosystem: >50% of top binary wheels are FT-compatible; asyncpg/uvloop/pydantic-core/msgspec/jiter green; psycopg/grpcio/orjson blocked (all with substitutes).

Three strategic options: (a) free-threaded only — caps the addressable market for years (no official Docker t-images, no managed cloud FT runtimes, ecosystem holes); (c) GIL-first — requires re-architecting in 2–3 years exactly when the project matures, and forfeits the one structural capability incumbents can't copy; (b) free-threaded-first with GIL compatibility.

## Decision

Option (b). All internals are thread-safe by construction (frozen pyclasses, `Sync` state, nothing relies on the GIL). Two worker topologies from one codebase: threads + loop-per-core on t-builds; process workers on GIL builds. Shared-state primitives (ADR: rayo-state) are genuinely shared on FT, same-API per-worker on GIL. Startup detects extensions that silently re-enable the GIL and names them.

## Consequences

- CI runs everything on both builds; a test passing on only one is a bug.
- FT is marketed as upside ("the first framework built for Python without the GIL"), never required.
- We carry a modest complexity tax (two topologies) in exchange for being *born ready* when the default flips.
