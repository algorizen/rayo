# Architecture Decision Records

Every decision with lasting consequences gets an ADR: short, dated, immutable once accepted. To change a decision, write a new ADR that supersedes the old one — history is part of the record.

Format: **Status / Context / Decision / Consequences**. Keep it under a page. Evidence beats opinion: cite the research, benchmark, or postmortem that motivated the choice.

## Index

| # | Title | Status |
|---|---|---|
| [0001](0001-rust-core-python-surface.md) | Rust core, Python surface, via PyO3 | Accepted |
| [0002](0002-free-threaded-first.md) | Free-threaded-first, GIL-compatible | Accepted |
| [0003](0003-embedded-server.md) | Embedded server; native protocol inside, ASGI adapter outside | Accepted |
| [0004](0004-custom-scheduler.md) | Custom coroutine scheduler, not generic async glue | Accepted |
| [0005](0005-eager-slotted-models.md) | Eager slotted models; Rust-direct response serialization | Accepted |
| [0006](0006-schema-ir-compilation.md) | Schema: Python introspection → flat IR → Rust, content-hash cached | Accepted |
| [0007](0007-no-custom-event-loop.md) | No custom asyncio event loop in v1; loop is pluggable | Accepted |
| [0008](0008-wheel-strategy.md) | Per-version wheels via maturin; abi3t later | Accepted |
| [0009](0009-no-nih-stack.md) | No ORM, no template engine, no DB drivers | Accepted |
| [0010](0010-license-and-governance.md) | MIT license; community governance from day one | Accepted |
