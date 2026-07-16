# Contributing to Rayo

Thank you for being here — especially this early. Pre-1.0 is when contributions shape a project most.

## The most important thing

**You do not need to know Rust to contribute to Rayo.** The Rust core is a minority of the surface. Docs, the Python API layer, integrations, examples, the conformance suite, error-message quality, and testing are all pure Python (or prose) and are the highest-leverage places to start.

## Ways to contribute

| Area | Skills | Where |
|---|---|---|
| Documentation (tutorials, reference, guides) | Prose, Python | `docs/` |
| Python surface (decorators, DI, bridge, testing utils) | Python | `python/rayo/` |
| Compatibility suite (migration-behavior tests) | Python, pytest | `conformance/` |
| Examples & templates | Python | `examples/` |
| Integrations (SQLAlchemy, auth, observability) | Python | `python/rayo/contrib/` |
| Rust core | Rust, PyO3 | `crates/` |
| Design | Thinking in public | [RFCs](docs/rfcs/) |

Issues labeled `good-first-issue` are curated to be genuinely finishable in an evening. Issues labeled `rust-core` include a pointer to the relevant section of [ARCHITECTURE.md](ARCHITECTURE.md).

## Development setup

```bash
# prerequisites: uv (https://docs.astral.sh/uv/), Rust toolchain (only for core work)
git clone https://github.com/algorizen/rayo && cd rayo
uv sync                    # Python env + dev dependencies + builds the Rust core
uv run pytest              # Python tests
uv run maturin develop     # rebuild just the Rust core while iterating on it
cargo test --workspace     # Rust tests (core work only)
```

To test on free-threaded Python:
`UV_PROJECT_ENVIRONMENT=.venv-ft uv sync --python 3.14t`, then prefix `uv run`
commands with the same variable (keeps the GIL-build venv in `.venv` intact).

## Pull requests

1. **Small and focused beats large and heroic.** If a change needs design discussion, open an issue or RFC first — it saves everyone time.
2. Every PR needs tests (or an explanation of why not) and passing CI.
3. Code follows [docs/code-standards.md](docs/code-standards.md). CI enforces `ruff`/`mypy` (Python) and `cargo fmt`/`clippy` (Rust).
4. Public API changes require a docs update in the same PR.
5. Anything touching the request hot path must respect the invariants in [ARCHITECTURE.md](ARCHITECTURE.md) §6 and say so in the PR description.

We commit to first review feedback within **7 days**. Stale-PR purgatory is a documented failure mode of this ecosystem; if we're slow, ping the thread — that's not rude, it's helping.

## Commit messages

Conventional-commit style: `type(scope): summary` — e.g. `feat(router): add path parameter constraints`, `fix(schema): reject NaN in float fields`. Describe the code change itself, not the process around it.

## RFCs

Changes to public API, wire behavior, or architecture invariants go through the lightweight [RFC process](docs/rfcs/README.md). Everything else is just a PR.

## Becoming a maintainer

Documented in [GOVERNANCE.md](GOVERNANCE.md). The short version: sustained quality contributions in an area → triage rights → merge rights in that area. We *want* to give commit access; single-maintainer projects die.

## Conduct

We follow the [Code of Conduct](CODE_OF_CONDUCT.md). Report issues to conduct@algorizen.ai.
