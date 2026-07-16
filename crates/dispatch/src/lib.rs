//! `rayo-dispatch` — the coroutine scheduler: direct coroutine stepping,
//! freelisted awaitables, oneshot-channel responses, one `Python::attach` per
//! request (invariant 1), and thread-per-request sync handlers on
//! free-threaded builds. See ADR-0004 and docs/design/dispatch-scheduler.md.
//!
//! Milestone 0 stub — the implementation lands in Milestone 1 (PLAN.md, W1).
//! Per code standards, this crate is where the project's `unsafe` FFI code
//! will be confined; every `unsafe` block requires a `// SAFETY:` comment.
