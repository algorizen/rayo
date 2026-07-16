## What does this change?

<!-- Describe the code change itself. Link the issue/RFC it implements. -->

## Hot-path invariant impact

<!-- Required if this touches the request path (server, router, dispatch,
schema, or the Python↔Rust boundary): state how the change respects the
invariants in ARCHITECTURE.md §6 — or write "does not touch the hot path". -->

## Checklist

- [ ] Tests pass on a GIL build and a free-threaded build (`uv run pytest`; CI covers the matrix)
- [ ] `cargo fmt`, `cargo clippy`, `ruff`, and `mypy` are clean
- [ ] Commits follow `type(scope): summary` and are signed off (DCO, `git commit -s`)
- [ ] Public-API changes have an accepted RFC; lasting design decisions have an ADR
