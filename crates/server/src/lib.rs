//! `rayo-server` — the embedded HTTP server: Tokio + Hyper, HTTP/1.1 and
//! HTTP/2, rustls, graceful shutdown, and the two worker topologies
//! (process workers on GIL builds, thread workers + loop-per-core on
//! free-threaded builds). See ADR-0003 and ADR-0006.
//!
//! Milestone 0 stub — the implementation lands in Milestone 1 (PLAN.md, W1).

#![forbid(unsafe_code)]
