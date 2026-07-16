# ADR-0001: Rust core, Python surface, via PyO3

**Status:** Accepted — 2026-07-16

## Context

The goal is a Python framework where users write only business logic, with the entire request lifecycle (server, routing, validation, serialization, jobs, state) handled natively. Candidate implementations: pure Python with C accelerators, Cython (tried in this category and abandoned), C++ via CFFI (likewise), or Rust via PyO3.

pydantic-core proved the model: a Rust core behind a Python API delivered 5–50x and the ecosystem embraced it. PyO3 is mature (0.29: free-threaded support, abi3t), maturin solves distribution, and Granian demonstrates a production-quality Rust HTTP stack serving Python at scale (adopted by Sentry). Rust's `Send`/`Sync` discipline is also the only practical way to deliver the free-threaded thread-safety guarantees in ADR-0002 — C extensions get no compiler help there.

## Decision

The core is Rust (Tokio + Hyper + PyO3 ≥0.29, `gil_used = false`); the user-facing surface is pure Python. The boundary rule: **one `Python::attach` per request; Python is entered only to run user handlers.** A PyO3 crossing costs ~30 ns and conversion is per-object, so the design minimizes objects crossing, not just calls.

## Consequences

- Users never need Rust installed (wheels — ADR-0008); contributors mostly don't either (the Python layer is the majority surface).
- The hot path is bound by our discipline, not the language: invariants in ARCHITECTURE.md §6 are review-enforced.
- We accept the build/CI complexity of a mixed repo and a wheel matrix as the cost.
