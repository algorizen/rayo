# Development Plan

The engineering execution plan: workstreams, milestones, task breakdown, and exit criteria. The public-facing summary is [ROADMAP.md](ROADMAP.md); the architecture is [ARCHITECTURE.md](ARCHITECTURE.md); decisions are recorded in [docs/adr/](docs/adr/).

## Non-goals

No ORM, template engine, DB drivers, or HTTP client (ADR-0009). No custom event loop in v1 (ADR-0007). No free-threading-only support (ADR-0002). No benchmark-leaderboard marketing (docs/benchmarks-policy.md). Not a fork of any existing framework.

## Workstreams

| # | Workstream | Owns |
|---|---|---|
| W1 | Runtime | `rayo-server`, `rayo-router`, `rayo-dispatch`, worker topologies |
| W2 | Schema | `rayo-schema`, models, IR, OpenAPI |
| W3 | Capabilities | `rayo-stream`, `rayo-jobs`, `rayo-state` |
| W4 | Surface | `python/rayo/` — decorators, DI, bridges, testing utils, errors |
| W5 | Quality | conformance suite, benchmarks harness, fuzzing, CI matrix |
| W6 | Community | docs site, examples, governance ops, release automation |

Dependency spine: W1 dispatch + W2 models are the critical path; W3 builds on both; W4 tracks W1–W3 continuously; W5/W6 never block on features (they start at M0).

---

## Milestone 0 — Repo bootstrap (target: +2 weeks)

- [ ] Reserve `rayo` + `rayoweb` on PyPI (placeholder 0.0.1); create `github.com/algorizen/rayo`; docs subdomain on algorizen.ai
- [x] Cargo workspace: `crates/{core,server,router,schema,dispatch}` stubs; `python/rayo/` package; maturin config; `uv` workflow
- [x] CI: lint (ruff/mypy/fmt/clippy), test on {Linux, macOS, Windows} × {3.10–3.14, 3.14t}; wheel-build dry run
- [ ] Release automation scripted end-to-end (tag → wheels → PyPI); one real 0.0.x release proves it *(workflow written; needs PyPI trusted publisher + first tag)*
- [x] Repo docs land (this repo's .md set); Discussions + issue templates on

**Exit:** `pip install <name>` works; `from <name> import <App>` imports; CI green on both interpreter builds.

## Milestone 1 — Request spine (target: +8 weeks)

- W1: Hyper server (h1/h2, rustls), graceful shutdown; radix router with path params; process-worker topology (GIL) + thread-worker topology (3.14t); dispatch scheduler v1 (PyIter_Send stepping, single attach, oneshot responses); sync-handler thread pool with first-class config
- W2: introspection → flat IR (native models, dataclasses); IR → Rust validators for JSON bodies, path/query params; slotted models with memoized getters; Rust-direct response serialization; 422 bodies generated in Rust
- W4: `App`, standalone route decorators, handler signature binding, startup diagnostics (schema/DI/GIL-re-enable checks)
- W5: dispatch micro-benchmark gating the <1 µs budget; cold-start suite skeleton; parser fuzz target

**Exit:** a CRUD app with typed models runs on GIL and 3.14t builds; hot-path invariants hold under test; `validate-db-serialize` benchmark runs against mainstream Python framework baselines (numbers private until the policy suite is complete).

## Milestone 2 — First-hour parity (target: +16 weeks)

- W2: pydantic-v2 model input bridge; OpenAPI 3.1 + Swagger/Scalar at `/docs`; content-hash schema cache
- W1: middleware layer; ASGI adapter (mounted apps + pure-ASGI middleware); cancellation matrix complete; blocking guard dev-mode (file:line) and prod-mode (metrics)
- W3: SSE + WebSockets with auth-equivalent DI, bounded-channel backpressure
- W4: DI with app/request scopes, startup-resolved graph, error-message quality pass; TestClient; type stubs gated in CI
- W5: conformance suite public with scoreboard; benchmark suites `cold-start`, `streaming`, `blocking-abuse` complete
- W6: docs site up — tutorial layer + generated API reference; llms.txt; 3+ example apps

**Exit:** the canonical getting-started journey (typed CRUD → auth → DI → docs UI) runs end-to-end with no gaps; compatibility scoreboard published; two external contributors have merged PRs.

## Milestone 3 — The new capabilities (target: +26 weeks)

- W3: durable jobs (`memory` + `sqlite` + `postgres` backends, leases, retries, DLQ, idempotency keys); detached/resumable streams over the jobs engine; `Pool`/`Cache`/`RateLimiter`/`Channel` with FT-shared + GIL-per-worker topologies
- W1: ops features — worker recycling, max-RSS limits, graceful reload, Prometheus metrics
- W3: MCP mounting (tools + resources) on the shared transport
- W5: `free-threaded` benchmark suite (cores-vs-throughput, shared pool); kill-restart job tests; FT concurrency stress suite
- W6: "structuring real applications" guide; deployment guides + Docker images (incl. 3.14t)

**Exit:** the three launch demos run scripted: (1) one process saturating all cores through a shared pool, (2) an agent stream surviving a browser refresh via `Last-Event-ID`, (3) the blocking guard naming a bug's file:line. Zero-loss job test passes under SIGKILL.

## Milestone 4 — Launch (target: Python 3.15 GA, Oct 2026)

- [ ] 0.x → beta versioning promise documented; API freeze for launch surface
- [ ] cp315 + abi3t wheels when maturin support lands (ADR-0008)
- [ ] Benchmark results page per policy (losses included) — goes live with launch
- [ ] Launch posts + demo videos (founder-led); Show HN/Reddit timed to 3.15 news cycle
- [ ] Governance gate: ≥2 maintainers with merge rights, funding rails live, security contact tested

**Exit:** launch week ends with the conformance scoreboard, benchmark page, and all three demos public; issues triaged within 48h.

---

## Standing rules

- PyPI never lags the repo (ADR-0008); ship every 1–2 weeks whatever the milestone state.
- Every merge to the hot path states its invariant impact (ARCHITECTURE.md §6) in the PR.
- Anything that would slip a milestone by >2 weeks gets re-scoped in the open (Discussion), not silently.
- RESEARCH.md (local, not published) holds the market evidence behind this plan; consult it before re-litigating settled decisions — then use an RFC if change is warranted.

## Risk watch (review monthly)

| Risk | Watch signal | Response |
|---|---|---|
| Critical-path slip on dispatch/schema | M1 exit slips >4 weeks | Cut M2 scope (defer ASGI adapter), never cut quality gates |
| Incumbent frameworks ship overlapping capability | Ecosystem release notes | Re-read RESEARCH.md moat ranking; our pillars need breaking changes incumbents can't make — verify that still holds |
| PyO3/maturin abi3t delays | maturin issue tracker | Ship cp315 version-specific wheels; abi3t is an optimization |
| Solo-maintainer regression | Bus factor stays 1 past M2 | Treat as launch blocker per ADR-0010 |
| FT ecosystem blocker resurfaces | Allowlist package loses FT support | Pin + upstream fix; document fallback |
