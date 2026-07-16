//! `rayo-schema` — compiles the flat schema IR produced by Python
//! introspection into validators and serializers: pull-parse JSON validation,
//! slotted model construction, and Rust-direct response serialization that
//! creates zero Python objects (invariant 3). See ADR-0005 and ADR-0006 and
//! docs/design/schema-compiler.md.
//!
//! Milestone 0 stub — the implementation lands in Milestone 1 (PLAN.md, W2).

#![forbid(unsafe_code)]
