# ADR-0006: Schema — Python introspection → flat IR → Rust compilation, content-hash cached

**Status:** Accepted — 2026-07-16

## Context

Every serious system does type introspection in the dynamic language and compiles in the native one (pydantic: metaclass → core-schema dict → Rust; msgspec: C reads type objects at decoder creation; serpyco-rs: `get_type_hints` → IR → Rust). Nobody makes Rust read `__annotations__` directly — Python typing churn (PEP 695, forward refs) is only tractable in Python.

The cautionary tale: pydantic v2's nested-PyObject schema walking produced documented 5s → 20s+ (extreme: 240s) app startups. That failure mode is structural: deep-copied nested graphs, no caching, eager builds.

## Decision

- Python-side introspection (handler signatures, native models, pydantic models, dataclasses, TypedDicts) emits a **flat, serializable IR**: postorder node list with integer child references — no nested PyObject graphs, recursion via explicit ref indices.
- Rust compiles the IR into validator/serializer trees in a single linear pass at startup.
- Compiled schemas are **cached by content hash across restarts** — unchanged models cost a hash lookup, making cold start O(changed schemas), not O(app size).
- Deferred build is supported from day one for tooling and tests.

## Consequences

- Sub-100ms cold starts on large apps become a testable product claim (`cold-start` benchmark suite).
- The IR is a versioned public-ish contract between the Python and Rust layers; changes need review against both sides plus cache-invalidation rules.
- Supporting a new schema source (e.g., attrs) is a pure-Python contribution — by design.
