# CLAUDE.md — AI assistant guide for the Rayo repository

Rayo is a Python web framework with a Rust core, designed free-threaded-first. Read this before making changes.

## What this project is

- **One-line thesis:** the process is the cluster, and work is durable. See [README.md](README.md) for vision, [PLAN.md](PLAN.md) for the development plan, [ARCHITECTURE.md](ARCHITECTURE.md) for the system, [docs/adr/](docs/adr/) for why each decision was made. (`RESEARCH.md`, when present, is local-only market evidence — gitignored, never link it from public docs.)
- Current phase: **Phase 0 (foundation)** — check [ROADMAP.md](ROADMAP.md) before building anything not listed there.

## Layout

- `crates/` — Rust core (server, router, schema, dispatch, stream, jobs, state). PyO3 ≥0.29, `gil_used = false`.
- `python/rayo/` — the Python surface: decorators, DI, type-hint introspection → IR, pydantic bridge, ASGI adapter, testing utils.
- `conformance/` — migration-compatibility tests (familiar framework behaviors). `benchmarks/` — Dockerized harness only; never commit numbers without it. Public docs never target other frameworks by name — criticism stays in local-only RESEARCH.md; technical library comparisons (pydantic-core, msgspec) are fine.
- `docs/adr/` — decision records; `docs/design/` — subsystem designs; `docs/rfcs/` — proposals.

## Hard invariants (violations are bugs, whatever the tests say)

1. One `Python::attach` per request on the hot path; `detach` around Rust work >1 ms.
2. No unbounded channels for Python-produced data; stream sends await capacity.
3. Zero Python object creation on the response serialization path.
4. Nothing relies on the GIL for correctness — all `#[pyclass]` frozen, shared state `Sync`. Code must pass tests on BOTH GIL and free-threaded (3.14t) builds.
5. Startup fails loudly and specifically; nothing defers config/schema/DI errors to request time.
6. Import-time Python does introspection only — no I/O, no schema compilation (Rust compiles the cached IR).

## Conventions

- Follow [docs/code-standards.md](docs/code-standards.md). Highlights: descriptive intention-revealing names everywhere (no `n`/`v`/`tmp`, no single-letter closure params); `// SAFETY:` on every `unsafe`; no `.unwrap()` reachable from request handling; `mypy --strict`-clean typed public API.
- Error messages are product: name what's wrong, where it was defined, and the fix.
- Commits: `type(scope): summary`, describing the code change itself — no process references, no validation status, no AI attribution trailers.
- Decisions with lasting consequences get an ADR (`docs/adr/`); public-API changes get an RFC first (`docs/rfcs/README.md`). Don't silently contradict an accepted ADR — propose superseding it.

## Working here

- Build: `uv run maturin develop`. Tests: `uv run pytest` (Python), `cargo test --workspace` (Rust). Free-threaded testing: `uv venv --python 3.14t`.
- When editing the hot path, state in the PR how the change respects the invariants above.
- Prior art to consult before inventing: Granian (scheduler, worker topology — MIT), pydantic-core (schema IR handoff), msgspec (slotted decode). ADRs cite the specific lessons.
- Do not add runtime Python dependencies (allowlist in code standards is empty) or new benchmark headline claims without the harness.
