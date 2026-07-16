# ADR-0008: Per-version wheels via maturin; abi3t later

**Status:** Accepted — 2026-07-16

## Context

abi3 (stable ABI) would shrink the wheel matrix but costs version-specific fast paths and PyO3 feature gaps — which is why the performance-critical Rust extensions all chose per-version wheels (pydantic-core: 121 wheels, no abi3; granian: 86 including t-builds; orjson: 61). There is **no stable ABI for free-threaded builds before Python 3.15**: abi3t (PEP 803, accepted March 2026) introduces it, with maturin tag support still landing.

Distribution failure modes from the ecosystem graveyard: C-extension build failures on user machines (the distribution tax kills adoption), and PyPI releases lagging the repository.

## Decision

- maturin, **per-version wheels**: cp310–cp314 + **cp314t**, manylinux2014 + musllinux, macOS universal2/arm64, Windows x64 (~60–90 wheels; scripted CI).
- Users never need Rust; sdist is a fallback, not a path we optimize.
- Adopt `abi3.abi3t` dual-tagging when Python 3.15 + maturin support matures — from then on, one FT wheel covers 3.15+ and the matrix shrinks.
- Release discipline is part of this ADR: **PyPI is never behind the repo**; releases are frequent, scripted, and boring (any `infra` maintainer can cut one).

## Consequences

- Significant but solved CI cost (granian's release workflow is the template).
- Version-specific optimizations in the validator hot path stay on the table.
- t-build users are first-class from the first release — nobody else in the category can say that.
