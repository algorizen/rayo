//! `rayo-router` — radix-tree routing with typed path parameters, built at
//! startup from the routes the Python surface registers (never mutated while
//! serving, so it is shared across worker threads without locks).
//!
//! Milestone 0 stub — the implementation lands in Milestone 1 (PLAN.md, W1).

#![forbid(unsafe_code)]
