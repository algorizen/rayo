# ADR-0010: MIT license; community governance from day one

**Status:** Accepted — 2026-07-16

## Context

Two documented failure modes bound this decision. (1) The solo-maintainer death: every dead or stalled project in this category was effectively single-author (90%+ commit concentration is the norm; one died when its maintainer was hired away, others stalled under visible maintainer fatigue) — versus uvloop (team-maintained, alive for a decade) and Litestar (org-governed, healthiest commit distribution in the field). (2) The BDFL bottleneck: the ecosystem's largest framework funnels every PR through one gatekeeper — a 20:1 commit ratio over the next human contributor, a heavily-upvoted "find maintainers" plea, and critical one-line fixes open for years; community frustration with this pattern is a matter of public record.

License: every framework in this ecosystem that achieved adoption is MIT/BSD/Apache; anything restrictive (SSPL-style) would kill contribution and corporate adoption for zero benefit at our stage.

## Decision

- **MIT license**, from the first commit, no CLA (DCO sign-off only).
- Governance per [GOVERNANCE.md](../../GOVERNANCE.md): area-scoped maintainership granted early, a 3–5 person steering group, lazy consensus, public RFC/ADR record, founder-disappearance escrow.
- Two-plus people with merge rights **before** launch marketing begins — this is a launch gate, equal in priority to any feature.
- Funding rails (GitHub Sponsors, Open Collective) open on day one, with transparent spending.

## Consequences

- We trade some early velocity (review, consensus) for survivability — the trade every failed competitor refused.
- The founder's marketing push has a durable structure to receive the community it attracts.
