# ADR-0005: Eager slotted models; Rust-direct response serialization

**Status:** Accepted — 2026-07-16

## Context

Where request latency actually lives in today's stacks: pydantic-core validates in Rust but still eagerly materializes a full Python object graph (every field, a `__dict__`, fields-set bookkeeping) — "~95% of JSON-load time is Python object creation, not parsing." msgspec beats pydantic v2 by ~12–15x by decoding **directly into typed, `__dict__`-less slots, validating during decode** — no intermediate dicts, no second pass.

We initially considered *lazy* materialization as the default (validate in Rust, materialize fields only on access — pysimdjson's proxy model). The evidence killed it as a default: validation must touch every leaf anyway; typical handlers read most fields; per-access FFI (~30 ns) plus proxy allocations exceed eager slot-filling; and simdjson-style proxies carry a document-lifetime invalidation trap that a web framework's users *will* hit by stashing a body past the request.

## Decision

- **Request path:** one-pass parse + validate (jiter-style pull parser) into **Rust-native slotted model objects**. Python field objects are created via **memoized getters on first access**, buffers Arc-owned by the model (no invalidation hazard). Laziness becomes an implementation detail, not a semantic — sparse-access workloads (webhooks reading 2 of 200 fields) get the win automatically.
- **Response path:** handlers return Rayo models (or bridged pydantic/dataclass values); serialization goes **Rust → JSON bytes directly. Zero Python objects are created for output.** This is the single largest uncontested win over every incumbent.

## Consequences

- Rayo models are not dicts and don't have `__dict__`; the pydantic bridge (ADR-0006 IR accepts pydantic models) covers compatibility.
- Response-side invariant (ARCHITECTURE.md §6.3) is review-enforced; features that would require materializing output (e.g., Python-side response middleware mutating bodies) must go through the ASGI adapter's documented cost instead.
