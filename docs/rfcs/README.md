# Rayo RFCs

Changes to public API, wire behavior, or architecture invariants (ARCHITECTURE.md §6) go through this process. Everything else is just a PR. The process is deliberately lightweight — its purpose is *thinking in public*, not bureaucracy.

## Process

1. **Optional pre-RFC:** open a Discussion to gauge whether an RFC is warranted. Maintainers will tell you honestly.
2. **Draft:** copy [0000-template.md](0000-template.md) to `NNNN-short-title.md` (next free number), fill it in, open a PR.
3. **Shepherd:** a maintainer volunteers as shepherd within 7 days — their job is to keep discussion moving and honest, not to advocate.
4. **Comment period:** minimum 14 days from PR open. Substantive revisions restart a 7-day clock.
5. **Disposition:** the shepherd proposes accept / reject / postpone; steering-group lazy consensus confirms. The reasoning is written into the RFC's header before merge. Rejected and postponed RFCs are merged too — the record is the point.
6. Accepted RFCs get a tracking issue; implementation PRs link back.

## What needs an RFC

- Anything a user's code can observe: decorator signatures, model semantics, error formats, wire behavior, configuration.
- Anything that changes an ADR's decision (the RFC produces a superseding ADR).
- New runtime dependencies, new crates, new primitives.

## What doesn't

Bugfixes, performance work within existing behavior, docs, internal refactors, additions to the conformance suite.

## Index

| # | Title | Status |
|---|---|---|
| — | *(none yet — yours could be first)* | |
