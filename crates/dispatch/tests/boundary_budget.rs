//! CI gate for the dispatch-boundary budget (ARCHITECTURE.md §6).
//!
//! Absolute wall-clock gates are meaningless on shared CI runners, so the
//! gate is a *ratio* measured in one process on one machine: Rayo's full
//! dispatch round-trip (schedule → coroutine stepped → response bytes over
//! the oneshot) versus asyncio's own `run_coroutine_threadsafe` round-trip
//! for the same trivial coroutine on a plain asyncio loop thread. Both cross
//! the thread boundary once and run identical handler code, so machine speed
//! cancels out of the ratio. The custom scheduler (ADR-0004) exists to beat
//! that generic glue; losing to it is exactly the regression to fail on.
//!
//! Ignored under a plain `cargo test` — timing assertions do not belong in
//! the default suite. CI runs it explicitly (the "Dispatch boundary budget"
//! job) on both a GIL and a free-threaded interpreter. The committed
//! `*_BASELINE_RATIO` constants below are the in-repo record of where each
//! build started; absolute medians print to the job log only and stay out
//! of public docs per the benchmarks policy.

// A benchmark harness fails by panicking with a message; the no-expect rule
// exists for the request path, not for test binaries.
#![allow(clippy::expect_used)]

use std::time::Instant;

use pyo3::prelude::*;
use pyo3::types::{PyDict, PyModule};
use rayo_dispatch::EventLoopPool;

const WARMUP_ROUND_TRIPS: usize = 300;
const ROUND_TRIPS_PER_BATCH: usize = 1500;
const BATCHES: usize = 7;
/// Hard ceiling: rayo's dispatch must beat the glue path by at least this
/// margin on any machine, whatever the committed baseline says.
const MAX_ALLOWED_RATIO: f64 = 0.85;

/// Baseline ratios measured on ubuntu-latest CI, 2026-07-17 (medians of
/// `BATCHES` interleaved batches; uv-managed interpreters, release build) —
/// the committed record of where each interpreter build started. A dispatch
/// change that intentionally moves a baseline updates the constant in the
/// same PR, citing the new CI numbers — never to quiet a noisy run.
const GIL_BASELINE_RATIO: f64 = 0.719;
const FREE_THREADED_BASELINE_RATIO: f64 = 0.419;
/// How far the measured ratio may drift above its committed baseline before
/// the gate fails. On the free-threaded build — where the scheduler's whole
/// advantage lives — this is far tighter than the 0.85 ceiling; wide enough
/// to absorb shared-runner noise while the job earns required-check status.
const MAX_REGRESSION_OVER_BASELINE: f64 = 1.25;

const BENCH_MODULE_SOURCE: &std::ffi::CStr = c"import asyncio\nimport threading\n\n\nasync def handler():\n    return None\n\n\ndef start_reference_loop():\n    reference_loop = asyncio.new_event_loop()\n    loop_thread = threading.Thread(\n        target=reference_loop.run_forever, name=\"reference-loop\", daemon=True\n    )\n    loop_thread.start()\n    return reference_loop\n";

fn median_seconds(samples: &mut [f64]) -> f64 {
    samples.sort_by(f64::total_cmp);
    samples[samples.len() / 2]
}

/// Seconds per round-trip through Rayo's scheduler: single attach, schedule
/// onto the pool, await the response bytes — the production dispatch path.
fn rayo_batch_seconds(
    pool: &EventLoopPool,
    handler: &Py<PyAny>,
    runtime: &tokio::runtime::Runtime,
    round_trips: usize,
) -> f64 {
    let started = Instant::now();
    // One runtime entry per batch, not per round-trip: production awaits
    // responses inside an already-running runtime, so a per-iteration
    // `block_on` would charge entry/exit glue to rayo alone.
    runtime.block_on(async {
        for _ in 0..round_trips {
            let mut dispatched = Python::attach(|py| pool.schedule(py, handler, PyDict::new(py)));
            let response = dispatched
                .response()
                .await
                .expect("rayo dispatch failed mid-benchmark: the scheduler dropped a request");
            // A failing handler also resolves the oneshot (with a 500), so
            // presence alone can't tell a working dispatch path from one that
            // errors on every request; the None-returning handler must
            // surface as 204.
            assert_eq!(
                response.status, 204,
                "rayo dispatch returned an error mid-benchmark instead of completing the handler"
            );
        }
    });
    started.elapsed().as_secs_f64() / round_trips as f64
}

/// Seconds per round-trip through asyncio's generic glue: build the
/// coroutine, `run_coroutine_threadsafe` onto the reference loop, block on
/// the concurrent.futures result (which detaches internally while waiting).
fn reference_batch_seconds(
    run_coroutine_threadsafe: &Py<PyAny>,
    handler: &Py<PyAny>,
    reference_loop: &Py<PyAny>,
    round_trips: usize,
) -> f64 {
    let started = Instant::now();
    for _ in 0..round_trips {
        Python::attach(|py| {
            let coroutine = handler
                .bind(py)
                .call0()
                .expect("building the reference coroutine failed");
            let pending_result = run_coroutine_threadsafe
                .bind(py)
                .call1((coroutine, reference_loop.bind(py)))
                .expect("run_coroutine_threadsafe refused the coroutine");
            pending_result
                .call_method1("result", (30.0,))
                .expect("the reference round-trip timed out");
        });
    }
    started.elapsed().as_secs_f64() / round_trips as f64
}

struct BenchObjects {
    handler: Py<PyAny>,
    reference_loop: Py<PyAny>,
    run_coroutine_threadsafe: Py<PyAny>,
    pool: EventLoopPool,
    free_threaded: bool,
}

/// Whether the embedded interpreter is a free-threaded build, which selects
/// the committed baseline the regression gate compares against.
fn interpreter_is_free_threaded(py: Python<'_>) -> PyResult<bool> {
    let gil_disabled = py
        .import("sysconfig")?
        .call_method1("get_config_var", ("Py_GIL_DISABLED",))?;
    // GIL builds report 0 (3.13+) or None (3.12); free-threaded builds 1.
    Ok(gil_disabled.extract::<i64>().unwrap_or(0) != 0)
}

#[test]
#[ignore = "timing gate; CI runs it explicitly via the dispatch-budget job"]
fn dispatch_stays_faster_than_asyncio_glue() {
    Python::initialize();
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .expect("building the tokio runtime failed");

    let bench = Python::attach(|py| -> PyResult<BenchObjects> {
        let bench_module = PyModule::from_code(
            py,
            BENCH_MODULE_SOURCE,
            c"boundary_budget.py",
            c"boundary_budget",
        )?;
        Ok(BenchObjects {
            handler: bench_module.getattr("handler")?.unbind(),
            reference_loop: bench_module.call_method0("start_reference_loop")?.unbind(),
            run_coroutine_threadsafe: py
                .import("asyncio")?
                .getattr("run_coroutine_threadsafe")?
                .unbind(),
            pool: EventLoopPool::start(py, 1)?,
            free_threaded: interpreter_is_free_threaded(py)?,
        })
    })
    .expect("benchmark setup failed");

    // Warm both paths: interned strings, freelists, allocator pools.
    rayo_batch_seconds(&bench.pool, &bench.handler, &runtime, WARMUP_ROUND_TRIPS);
    reference_batch_seconds(
        &bench.run_coroutine_threadsafe,
        &bench.handler,
        &bench.reference_loop,
        WARMUP_ROUND_TRIPS,
    );

    // Interleave batches so drift (thermal, noisy neighbors) hits both paths
    // roughly equally; the median batch is the figure of record.
    let mut rayo_samples = Vec::with_capacity(BATCHES);
    let mut reference_samples = Vec::with_capacity(BATCHES);
    for _ in 0..BATCHES {
        rayo_samples.push(rayo_batch_seconds(
            &bench.pool,
            &bench.handler,
            &runtime,
            ROUND_TRIPS_PER_BATCH,
        ));
        reference_samples.push(reference_batch_seconds(
            &bench.run_coroutine_threadsafe,
            &bench.handler,
            &bench.reference_loop,
            ROUND_TRIPS_PER_BATCH,
        ));
    }
    let rayo_median = median_seconds(&mut rayo_samples);
    let reference_median = median_seconds(&mut reference_samples);
    let overhead_ratio = rayo_median / reference_median;

    let (baseline_label, baseline_ratio) = if bench.free_threaded {
        ("free-threaded", FREE_THREADED_BASELINE_RATIO)
    } else {
        ("GIL", GIL_BASELINE_RATIO)
    };
    let regression_gate = baseline_ratio * MAX_REGRESSION_OVER_BASELINE;

    println!(
        "rayo dispatch round-trip:  {:8.2} µs (median of {BATCHES} batches)",
        rayo_median * 1e6
    );
    println!(
        "run_coroutine_threadsafe:  {:8.2} µs (median of {BATCHES} batches)",
        reference_median * 1e6
    );
    println!(
        "ratio (rayo / reference):  {overhead_ratio:.3}   gates: <= {MAX_ALLOWED_RATIO} \
         (ceiling), <= {regression_gate:.3} ({baseline_label} baseline {baseline_ratio} \
         x {MAX_REGRESSION_OVER_BASELINE})"
    );

    Python::attach(|py| bench.pool.stop(py));

    assert!(
        overhead_ratio <= MAX_ALLOWED_RATIO,
        "dispatch-boundary regression: rayo's round-trip is {overhead_ratio:.3}x the \
         asyncio run_coroutine_threadsafe path (ceiling: {MAX_ALLOWED_RATIO}). The custom \
         scheduler exists to beat that glue — profile the dispatch path before merging."
    );
    assert!(
        overhead_ratio <= regression_gate,
        "dispatch-boundary regression: ratio {overhead_ratio:.3} drifted more than \
         {MAX_REGRESSION_OVER_BASELINE}x above the committed {baseline_label} baseline \
         ({baseline_ratio}). Profile the dispatch path; if the change is intentional, \
         update the baseline constant in this file citing the new CI numbers."
    );
}
