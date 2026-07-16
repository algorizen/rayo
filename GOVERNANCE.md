# Rayo Governance

Rayo is an open-source, community-governed project under the MIT license. This document describes how decisions are made and how people gain responsibility. It is deliberately lightweight and will evolve by RFC.

## Design principle

The single most common cause of death for projects in this space is the **solo-maintainer bottleneck** — either the maintainer burns out, gets hired away, or becomes the review chokepoint that leaves PRs rotting for years. Rayo's governance is designed against that failure mode from day one.

## Roles

**Contributor** — anyone who participates: code, docs, issues, RFC review, community help.

**Triager** — can label, close, and shepherd issues/PRs in their area. Granted after sustained constructive participation.

**Maintainer** — merge rights in one or more areas (`python-surface`, `rust-core`, `docs`, `conformance`, `infra`). Granted by consensus of existing maintainers after a track record of quality contributions *and reviews* in that area. Area-scoped rights mean we can hand out responsibility early without handing out risk.

**Steering group** — 3–5 maintainers (including the founder) who own the roadmap, releases, security response, and conduct enforcement. Decisions by lazy consensus; a vote (simple majority) only when consensus fails after honest effort.

## How decisions are made

1. **Trivial changes** (bugfixes, docs, refactors): PR review by one maintainer of the area.
2. **Public API / wire behavior / architecture invariants**: [RFC](docs/rfcs/README.md). RFCs get a named shepherd, a comment period of at least 14 days, and an explicit disposition (accept / reject / postpone) with reasons recorded in the RFC file.
3. **Irreversible or project-level decisions** (license, governance, release policy): steering group, in public, with an ADR recorded in [docs/adr/](docs/adr/).

Design happens in public. Private channels may exist for coordination and security, but no design decision is real until it is written down in an issue, RFC, or ADR.

## Releases

- Semantic versioning. Pre-1.0: minor versions may break, patch versions never do; breaking changes are always listed first in release notes.
- **The repo is never ahead of PyPI** by more than the current development cycle — releases are frequent and boring.
- Any maintainer of `infra` can cut a release; the process is fully scripted and documented.

## Funding

GitHub Sponsors and Open Collective, open from day one. Funds are spent transparently (CI, infrastructure, contributor grants) and reported in the Open Collective ledger. Money never buys design decisions; sponsorship is acknowledged, not obeyed.

## The founder's role and stewardship

The founder (project creator) leads marketing and community growth. **Algorizen** (algorizen.ai, a UAE-based LLC owned by the founder) acts as the project's steward: it hosts the GitHub organization and holds the trademark, domain, and PyPI credentials in trust for the project. Stewardship is custody, not control: on design, the founder is one maintainer among several — the RFC process binds everyone, including Algorizen, equally, and the MIT license guarantees the community's right to fork. If the founder is unreachable for 90+ days, the steering group can act with full authority, including credentials recovery via the pre-arranged escrow.

## Amendments

This document changes by RFC with steering-group approval.
