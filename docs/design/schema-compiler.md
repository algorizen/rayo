# Design: Schema Compiler and Models

**Crate:** `rayo-schema` + `python/rayo/introspect.py` · **Related:** ADR-0005, ADR-0006 · **Status:** Draft

## Goals

Type hints → validation, serialization, and OpenAPI, compiled once, cached across restarts. msgspec-class request performance, pydantic-class ergonomics, zero Python objects on the response path.

## Non-goals

Being a general-purpose validation library (Rayo-internal first; extraction to a standalone package is a possible later RFC). Runtime schema mutation.

## Design

### Sources → IR (Python, import time)

`rayo.introspect` reads handler signatures and model definitions from: Rayo native models, Pydantic v2 models, dataclasses, TypedDicts, plain annotations (`int`, `Annotated[str, MaxLen(80)]`, unions, generics). Output: the **flat IR** — a postorder array of nodes `{kind, params, child_refs: [int]}`, recursion via explicit indices, serialized to bytes (msgpack). Import-time budget: introspection only — no I/O, no compilation.

### IR → validators (Rust, startup)

Single linear pass builds a validator/serializer arena (enum dispatch, no boxing per node where avoidable). The compiled artifact is cached under `content_hash(IR bytes + rayo-schema version)` in `$RAYO_CACHE` — unchanged models cost one hash lookup. Deferred build supported (`compile=False` for tooling/tests).

### Request path

jiter-style pull parser drives validation in one pass: `next_key()` against a compiled key-lookup (interned key cache), leaf validation during parse, output written directly into a **Rust slotted model** (fields as Rust values; no `PyDict`, no `__dict__`). Errors accumulate with JSON-pointer paths and produce the 422 body *in Rust*.

### Models in Python

`#[pyclass(frozen)]` with per-field **memoized getters**: first access materializes the Python object (30 ns FFI + conversion), subsequent accesses hit the memo. Buffers are `Arc`-owned by the model — stashing a model or field past the request is safe (no simdjson-style invalidation). Equality/`repr`/`__match_args__` provided; mutation is out (models are values; derive-with-changes via `model.replace(...)`).

### Response path

Handlers return Rayo models, bridged pydantic/dataclass values, or primitives. The serializer walks Rust-side state; memoized Python fields that were *replaced* (via `replace`) serialize from their new values. **Invariant: no Python object creation during serialization** — enforced by a debug-mode allocation hook in tests.

### OpenAPI

Generated from the same IR (single source of truth) at startup; JSON Schema 2020-12 output; Swagger/Scalar UIs serve static artifacts.

## Open questions

1. IR versioning policy and cache invalidation across Rayo upgrades.
2. Custom validator escape hatch: Python callables per field (crossing cost documented) vs a restricted Rust-side expression set — likely both, in that order.
3. `attrs` support priority (pure-Python contribution by design).

## Test strategy

Property-based round-trip tests (hypothesis: value → JSON → model → JSON); conformance vs pydantic v2 semantics where we claim compatibility; the cold-start benchmark suite (10/100/1000 routes) gating O(changed-schemas) claims; fuzzing the parser (CI, per SECURITY.md).
