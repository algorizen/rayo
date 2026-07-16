//! `rayo-dispatch` — the Rust↔Python dispatch boundary. See ADR-0004 and
//! docs/design/dispatch-scheduler.md.
//!
//! v1 topology: one Python thread runs an asyncio event loop; the Rust side
//! schedules handler coroutines onto it with `call_soon_threadsafe` and gets
//! the response back through a oneshot channel. Async handlers currently ride
//! `loop.create_task` — this is the stepping stone the ADR-0004 custom
//! scheduler (direct coroutine stepping, freelisted awaitables) replaces
//! within M1. Boundary invariants hold already: the request path enters
//! Python exactly once, and responses are serialized to bytes in Rust while
//! reading (never creating) Python objects.
//!
//! Per code standards, this crate is where the project's `unsafe` FFI code
//! will be confined; every `unsafe` block requires a `// SAFETY:` comment.
//! (v1 contains none.)

use std::sync::Mutex;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::thread::JoinHandle;

use pyo3::prelude::*;
use pyo3::types::{PyDict, PyString};
use tokio::sync::oneshot;

/// A handler's outcome, already reduced to wire data — nothing Python-shaped
/// crosses back toward the server.
pub struct HandlerResponse {
    pub status: u16,
    pub content_type: &'static str,
    pub body: Vec<u8>,
}

impl HandlerResponse {
    pub fn internal_error() -> Self {
        Self {
            status: 500,
            content_type: "text/plain; charset=utf-8",
            body: b"Internal Server Error".to_vec(),
        }
    }

    pub fn no_content() -> Self {
        Self {
            status: 204,
            content_type: "text/plain; charset=utf-8",
            body: Vec::new(),
        }
    }
}

/// Convert whatever the handler returned into wire data. Reads Python
/// objects; creates none (invariant 3).
fn response_from_return_value(value: &Bound<'_, PyAny>) -> HandlerResponse {
    if value.is_none() {
        return HandlerResponse::no_content();
    }
    if let Ok(text) = value.cast::<PyString>() {
        return match text.to_str() {
            Ok(body_text) => HandlerResponse {
                status: 200,
                content_type: "text/plain; charset=utf-8",
                body: body_text.as_bytes().to_vec(),
            },
            Err(encoding_error) => report_handler_error(value.py(), encoding_error),
        };
    }
    let mut body = Vec::with_capacity(128);
    match rayo_schema::write_json(value, &mut body) {
        Ok(()) => HandlerResponse {
            status: 200,
            content_type: "application/json",
            body,
        },
        Err(serialization_error) => report_handler_error(value.py(), serialization_error),
    }
}

/// Log the failure with its traceback and produce the opaque 500. Error
/// *bodies* stay generic; error *logs* carry everything.
fn report_handler_error(py: Python<'_>, error: PyErr) -> HandlerResponse {
    error.print(py);
    HandlerResponse::internal_error()
}

/// Called from Python when a handler's task finishes; forwards the outcome to
/// the waiting server task. Frozen: shared across threads without the GIL.
#[pyclass(frozen)]
pub struct CompletionCallback {
    response_sender: Mutex<Option<oneshot::Sender<HandlerResponse>>>,
}

impl CompletionCallback {
    fn send(&self, response: HandlerResponse) {
        let mut sender_slot = self
            .response_sender
            .lock()
            .unwrap_or_else(|poisoned_lock| poisoned_lock.into_inner());
        if let Some(sender) = sender_slot.take() {
            // The receiver disappearing just means the client went away.
            let _ = sender.send(response);
        }
    }
}

#[pymethods]
impl CompletionCallback {
    fn __call__(&self, finished_task: &Bound<'_, PyAny>) {
        let response = match finished_task.call_method0("result") {
            Ok(return_value) => response_from_return_value(&return_value),
            Err(handler_error) => report_handler_error(finished_task.py(), handler_error),
        };
        self.send(response);
    }
}

/// The Python event-loop thread and the handles needed to schedule onto it.
pub struct EventLoop {
    loop_object: Py<PyAny>,
    spawn_helper: Py<PyAny>,
    thread: Mutex<Option<JoinHandle<()>>>,
}

impl EventLoop {
    /// Create an asyncio event loop and run it forever on a dedicated thread.
    pub fn start(py: Python<'_>, thread_name: String) -> PyResult<Self> {
        let asyncio = py.import("asyncio")?;
        let loop_object = asyncio.call_method0("new_event_loop")?;
        let spawn_helper = py.import("rayo._runtime")?.getattr("spawn_handler")?;

        let loop_for_thread: Py<PyAny> = loop_object.clone().unbind();
        let python_visible_name = thread_name.clone();
        let thread = std::thread::Builder::new()
            .name(thread_name)
            .spawn(move || {
                Python::attach(|thread_py| {
                    // Foreign threads show up in Python as "Dummy-N"; give
                    // this one its real name so tracebacks and diagnostics
                    // point somewhere meaningful.
                    let renamed = thread_py
                        .import("threading")
                        .and_then(|threading| threading.call_method0("current_thread"))
                        .and_then(|current_thread| {
                            current_thread.setattr("name", python_visible_name)
                        });
                    if let Err(rename_error) = renamed {
                        rename_error.print(thread_py);
                    }
                    let event_loop = loop_for_thread.bind(thread_py);
                    if let Err(loop_error) = event_loop.call_method0("run_forever") {
                        loop_error.print(thread_py);
                    }
                    if let Err(close_error) = event_loop.call_method0("close") {
                        close_error.print(thread_py);
                    }
                });
            })?;

        Ok(Self {
            loop_object: loop_object.unbind(),
            spawn_helper: spawn_helper.unbind(),
            thread: Mutex::new(Some(thread)),
        })
    }

    /// Schedule an async handler onto the event loop. Always resolves the
    /// returned channel — scheduling failures become 500s, never hangs.
    pub fn schedule(
        &self,
        py: Python<'_>,
        handler: &Py<PyAny>,
        handler_kwargs: Bound<'_, PyDict>,
    ) -> oneshot::Receiver<HandlerResponse> {
        let (response_sender, response_receiver) = oneshot::channel();
        let completion = match Py::new(
            py,
            CompletionCallback {
                response_sender: Mutex::new(Some(response_sender)),
            },
        ) {
            Ok(callback) => callback,
            Err(allocation_error) => {
                allocation_error.print(py);
                return response_receiver; // sender dropped → receiver errors → 500
            }
        };

        let event_loop = self.loop_object.bind(py);
        let scheduled = event_loop.call_method1(
            "call_soon_threadsafe",
            (
                self.spawn_helper.bind(py),
                event_loop,
                handler.bind(py),
                handler_kwargs,
                &completion,
            ),
        );
        if let Err(scheduling_error) = scheduled {
            scheduling_error.print(py);
            completion.get().send(HandlerResponse::internal_error());
        }
        response_receiver
    }

    /// Stop the loop and join its thread. Idempotent.
    pub fn stop(&self, py: Python<'_>) {
        let event_loop = self.loop_object.bind(py);
        if let Ok(stop_method) = event_loop.getattr("stop") {
            if let Err(stop_error) = event_loop.call_method1("call_soon_threadsafe", (stop_method,))
            {
                stop_error.print(py);
            }
        }
        let thread_handle = {
            let mut thread_slot = self
                .thread
                .lock()
                .unwrap_or_else(|poisoned_lock| poisoned_lock.into_inner());
            thread_slot.take()
        };
        if let Some(handle) = thread_handle {
            // Detach so the loop thread can re-attach to finish shutting down.
            py.detach(|| {
                if handle.join().is_err() {
                    eprintln!("rayo-dispatch: event-loop thread panicked during shutdown");
                }
            });
        }
    }
}

/// N event loops on N threads, one intended per core on free-threaded builds
/// (where they run Python truly in parallel) and one total on GIL builds.
/// Requests distribute round-robin; the pool is `Sync` and lock-free on the
/// scheduling path (invariant 4).
pub struct EventLoopPool {
    loops: Vec<EventLoop>,
    next_loop_index: AtomicUsize,
}

impl EventLoopPool {
    /// Start `size` loop threads. `size == 0` is a startup error — reported
    /// immediately and specifically (invariant 5), never deferred.
    pub fn start(py: Python<'_>, size: usize) -> PyResult<Self> {
        if size == 0 {
            return Err(pyo3::exceptions::PyValueError::new_err(
                "loop_threads must be at least 1 (use None to let Rayo pick: \
                 one per core on free-threaded builds, one total on GIL builds)",
            ));
        }
        let mut loops = Vec::with_capacity(size);
        for loop_index in 0..size {
            loops.push(EventLoop::start(py, format!("rayo-loop-{loop_index}"))?);
        }
        Ok(Self {
            loops,
            next_loop_index: AtomicUsize::new(0),
        })
    }

    pub fn size(&self) -> usize {
        self.loops.len()
    }

    /// Schedule onto the next loop, round-robin.
    pub fn schedule(
        &self,
        py: Python<'_>,
        handler: &Py<PyAny>,
        handler_kwargs: Bound<'_, PyDict>,
    ) -> oneshot::Receiver<HandlerResponse> {
        let loop_index = self.next_loop_index.fetch_add(1, Ordering::Relaxed) % self.loops.len();
        self.loops[loop_index].schedule(py, handler, handler_kwargs)
    }

    /// Stop every loop and join its thread. Idempotent.
    pub fn stop(&self, py: Python<'_>) {
        for event_loop in &self.loops {
            event_loop.stop(py);
        }
    }
}

/// Run a sync handler to completion on the current thread (the caller is
/// responsible for putting this on a blocking-capable thread).
pub fn execute_sync_handler(
    handler: &Bound<'_, PyAny>,
    handler_kwargs: &Bound<'_, PyDict>,
) -> HandlerResponse {
    match handler.call((), Some(handler_kwargs)) {
        Ok(return_value) => response_from_return_value(&return_value),
        Err(handler_error) => report_handler_error(handler.py(), handler_error),
    }
}
