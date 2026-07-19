# Code Standards

Enforced by CI (`ruff`, `mypy --strict`, `cargo fmt --check`, `cargo clippy -D warnings`). This document covers what tooling can't.

## Both languages

- **Descriptive, intention-revealing names.** No single-letter or cryptic throwaway names (`n`, `v`, `tmp`, `data2`) — name the thing it holds (`route_table`, `validator_index`, `pending_jobs`). Loop and callback parameters get real names too (`.map(|worker| …)`, not `.map(|w| …)`). Conventional short idioms are acceptable only where unambiguous (`i` for a trivial counter, `err` in a catch/match arm) — prefer the full word.
- **Comments state constraints the code can't show** — invariants, safety arguments, protocol requirements. Never narrate what the next line does, and never reference the PR/review/plan that produced the code.
- **Errors are product surface.** Every user-facing error names the thing that's wrong, where it was defined, and what to do about it. "Startup fails loudly and specifically" is an architecture invariant; vague errors are bugs.
- **No dead code, no speculative abstraction.** We build what the roadmap needs.

## Rust (`crates/`)

- Edition 2024, `rustfmt` defaults, `clippy::pedantic` as warnings triaged in review.
- `unsafe` requires a `// SAFETY:` comment explaining the invariant, and is confined to `rayo-dispatch` (FFI) and explicitly-reviewed hot paths. New `unsafe` outside those needs maintainer sign-off.
- **Hot-path rules** (request lifecycle — see ARCHITECTURE.md §6):
  - No allocation in steady state where a freelist/buffer reuse is practical.
  - No unbounded channels for Python-produced data.
  - One `Python::attach` per request; `Python::detach` around any Rust work > 1 ms.
  - No `.unwrap()`/`.expect()` reachable from request handling — errors map to HTTP responses or structured logs.
- All `#[pyclass]` types are frozen unless an ADR says otherwise; interior mutability via `Sync` primitives only. Nothing may rely on the GIL for correctness (free-threaded builds are first-class).
- Public crate APIs get doc comments with an example; `cargo doc` must build clean.

## Python (`python/rayo/`)

- Ruff (lint + format), line length 100. `mypy --strict` on the package; the public API surface is fully typed, including overloads for decorator forms.
- **Type stubs are product.** Editor autocompletion is a tested feature — stub regressions are release blockers.
- Python 3.10+ syntax; no runtime dependencies beyond the compiled core except where an ADR grants one (current allowlist: none).
- Import-time work is budgeted: module import does introspection and IR building only — no I/O, no network, no schema *compilation* (that's Rust's job, cached).
- Async code never calls blocking I/O; the test suite runs with the blocking-call guard in strict mode, so violations fail tests.

## Tests

- Rust: unit tests per crate; boundary behavior integration-tested from Python (that's what users experience).
- Python: pytest; every public behavior has a test; conformance suite (`conformance/`) runs on every PR.
- Both run against GIL and free-threaded interpreters in CI. A test that only passes on one build is a bug.
- Benchmarks are not tests: they live in `benchmarks/` and follow the [benchmark policy](benchmarks-policy.md). One carve-out: an internal CI regression gate may live in a crate's `tests/` when it is `#[ignore]`d under a plain `cargo test`, gates on a machine-cancelling ratio rather than wall-clock time, and publishes no absolute numbers (e.g. the dispatch boundary-budget gate).

## Commit messages

- Conventional-commit style: `type(scope): summary` (`feat`, `fix`, `perf`, `docs`, `refactor`, `test`, `ci`, `chore`).
- PR titles and issue titles follow the same `type(scope): summary` convention.
- Describe the code change itself — never the plan, discussion, or review process around it, and no validation status ("tests pass") in the message.
- No AI/tool attribution trailers.
- History rewriting: free before a PR exists; after that, follow-up commits are preferred and force-pushes (`--force-with-lease` only) are reserved for rewrites that genuinely improve the record. `main` is never rewritten.
