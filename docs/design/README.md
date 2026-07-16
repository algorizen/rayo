# Design Documents

Living technical designs for Rayo's subsystems — deeper than [ARCHITECTURE.md](../../ARCHITECTURE.md), narrower than the code. Unlike ADRs (immutable decisions), design docs evolve with implementation; substantive changes to the *decisions* inside them still require an RFC or superseding ADR.

| Document | Subsystem | Status |
|---|---|---|
| [dispatch-scheduler.md](dispatch-scheduler.md) | Coroutine scheduler, boundary, blocking guard | Draft |
| [schema-compiler.md](schema-compiler.md) | Type hints → IR → validators; models | Draft |
| [durable-jobs.md](durable-jobs.md) | Job queue, persistence, delivery semantics | Draft |
| [shared-state.md](shared-state.md) | Pool / Cache / RateLimiter / Channel | Draft |
| [streaming.md](streaming.md) | SSE, WebSockets, detached streams, backpressure | Draft |

Each doc states: goals, non-goals, the design, open questions, and test strategy. Open questions are invitations — pick one up in Discussions.
