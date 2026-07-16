# Design: Streaming, SSE, WebSockets, Detached Streams

**Crate:** `rayo-stream` · **Related:** ADR-0003, ADR-0004 · **Status:** Draft

## Goals

First-class primitives for the agent-era workload: per-token streaming, long-lived connections, correct disconnect semantics *in both directions*, end-to-end backpressure, and streams that can outlive their client. Websocket auth that just works.

## Non-goals

A message broker; HTTP/3 (blocked upstream on Hyper; revisit when it lands).

## Design

### Backpressure (the load-bearing rule)

Every Python-produced byte stream flows through a **bounded channel (default 8 chunks) whose `send` future resolves only when capacity frees**. Hyper/TCP flow control thus becomes a visible, awaitable signal in Python — a fast generator with a slow client *waits* instead of buffering unboundedly (the documented gap in granian's HTTP path we deliberately close). One boundary crossing per chunk is fine (~30 ns vs µs-scale writes).

### SSE

Built in: `@get("/events")` returning an async generator of `Event(data=…, id=…, event=…)` streams as `text/event-stream` with correct headers, heartbeat comments (configurable), and rate-limited flush coalescing.

**Disconnect semantics are per-route and explicit:**
- `on_disconnect=Cancel` (default): generator receives `CancelledError`, cleanup runs.
- `on_disconnect=Detach`: the generator keeps running as a durable-jobs-backed execution; events append to an event log; reconnecting clients send `Last-Event-ID` and resume from that offset. TTL and log bounds configured per route. This is the "agent run survives a browser refresh" primitive.

### WebSockets

- Auth dependencies work identically to HTTP routes (a long-standing gap across the ecosystem): the DI request scope exists at handshake, security schemes included.
- Rust handles ping/pong, close handshakes, and frame limits. Receive side: bounded queue (policy: block sender via WS flow control) — slow consumers can't balloon memory. Send side: awaits the actual socket write (true per-message backpressure).
- `rayo.Channel` integration: `room = channel.subscribe("doc:42")` + built-in connection manager — the tutorial `ConnectionManager` class everyone hand-rolls becomes a framework feature.

### MCP

MCP server endpoints mount with the same decorator surface (`@app.mcp_tool(...)`), sharing DI, auth, and the SSE transport machinery. Scope for v1: tools + resources over SSE/streamable HTTP; the point is that REST and MCP are one app, one auth story, one deploy.

### Cancellation interplay

Detached mode swaps the dispatch cancellation token (see dispatch-scheduler design) for a completion-tracked job handle. Everything else follows the scheduler's cancellation matrix.

## Open questions

1. Event-log storage for detached streams: always the jobs backend, or a dedicated ring buffer for `memory` mode?
2. WS message schema validation (typed `receive[Model]()`) in v1 or later?
3. Heartbeat defaults vs proxy timeout folklore (document the nginx/ALB numbers).

## Test strategy

Slow-client harness asserting bounded memory under firehose generators; disconnect matrix (each phase × Cancel/Detach); resume-from-`Last-Event-ID` correctness incl. process restart with persistent backend; websocket auth conformance vs HTTP routes; soak test for long-lived connections on both builds.
