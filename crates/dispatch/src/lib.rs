//! `rayo-dispatch` — the Rust↔Python dispatch boundary. See ADR-0004 and
//! docs/design/dispatch-scheduler.md.
//!
//! Async handlers are driven by [`HandlerTask`], a frozen task-like pyclass
//! that steps the handler coroutine directly with `PyIter_Send` — no
//! `asyncio.Task` is created per request. Yielded futures get the task as a
//! done callback; bare yields take one trip through the loop via `call_soon`;
//! cross-thread entry (initial dispatch, disconnect cancellation) is the only
//! use of `call_soon_threadsafe`. The design is ported from Granian's
//! MIT-licensed scheduler (credited in ADR-0004).
//!
//! Because no `asyncio.Task` exists, everything Task provides is reproduced
//! deliberately:
//!
//! - every step runs inside a per-request `contextvars.Context` and between
//!   `_enter_task`/`_leave_task`, and tasks register with `_register_task`,
//!   so `asyncio.current_task()`, `asyncio.all_tasks()`, `asyncio.timeout`,
//!   and `TaskGroup` behave as under `asyncio.Task`;
//! - the coroutine and context are built lazily on the loop thread (first
//!   step), so decorator wrappers that touch the running loop before
//!   returning the coroutine keep working;
//! - each loop keeps a registry of its live tasks: shutdown cancels them (so
//!   `finally` cleanup runs) and anything left when the loop dies is closed
//!   and disposed rather than leaked;
//! - `HandlerTask` implements `__traverse__` so the task ↔ future reference
//!   cycle a parked request forms stays visible to CPython's cycle GC.
//!
//! Boundary invariants: the request path enters Python exactly once, and
//! responses are serialized to bytes in Rust while reading (never creating)
//! Python objects.
//!
//! Known limitations, accepted deliberately: a task disposed after its loop
//! died gets synchronous cleanup only (`coroutine.close()` — awaiting
//! cleanup cannot run without a loop, and the failure is logged), and
//! [`DispatchedRequest`]'s drop-cancellation must attach to Python, which is
//! unsafe during interpreter finalization — mitigated by the finished-task
//! fast path (after `stop()` every task is finished, so teardown drops never
//! attach), not eliminated.
//!
//! Per code standards, this crate is where the project's `unsafe` FFI code is
//! confined; every `unsafe` block carries a `// SAFETY:` comment.

use std::collections::HashMap;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex, MutexGuard};
use std::thread::JoinHandle;

use pyo3::exceptions::asyncio::{CancelledError, InvalidStateError};
use pyo3::exceptions::{PyRuntimeError, PyStopIteration};
use pyo3::ffi;
use pyo3::intern;
use pyo3::prelude::*;
use pyo3::types::{PyDict, PyString};
use pyo3::{PyTraverseError, PyVisit};
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

/// The Rust half of a request's completion: the oneshot back to the server
/// task plus the owning loop's in-flight gauge. `send` fires at most once;
/// if the channel is dropped without ever sending, `Drop` still balances the
/// gauge and the dropped sender surfaces as a 500 on the receiver side.
struct ResponseChannel {
    response_sender: Mutex<Option<oneshot::Sender<HandlerResponse>>>,
    in_flight: Arc<AtomicUsize>,
}

impl ResponseChannel {
    fn send(&self, response: HandlerResponse) {
        let taken_sender = {
            let mut sender_slot = self
                .response_sender
                .lock()
                .unwrap_or_else(|poisoned_lock| poisoned_lock.into_inner());
            sender_slot.take()
        };
        if let Some(sender) = taken_sender {
            // `take` fires at most once per request, so this decrement
            // exactly balances the increment in `EventLoop::schedule`.
            self.in_flight.fetch_sub(1, Ordering::Relaxed);
            // The receiver disappearing just means the client went away.
            let _ = sender.send(response);
        }
    }
}

impl Drop for ResponseChannel {
    fn drop(&mut self) {
        let sender_slot = self
            .response_sender
            .get_mut()
            .unwrap_or_else(|poisoned_lock| poisoned_lock.into_inner());
        if sender_slot.take().is_some() {
            self.in_flight.fetch_sub(1, Ordering::Relaxed);
        }
    }
}

/// Handles shared by every task on one event loop, plus the loop's registry
/// of live tasks. The registry is what makes shutdown honest: it is the
/// cancellation list for graceful drain, and it keeps abandoned tasks
/// reachable until [`EventLoop::stop`] disposes of them.
///
/// Deliberately *not* visited from `HandlerTask::__traverse__`: these `Py`
/// references are single edges owned by this shared struct, and reporting
/// them from every task would over-count them to the cycle GC.
struct LoopHandles {
    loop_object: Py<PyAny>,
    enter_task: Py<PyAny>,
    leave_task: Py<PyAny>,
    register_task: Py<PyAny>,
    unregister_task: Py<PyAny>,
    live_tasks: Mutex<HashMap<usize, Py<HandlerTask>>>,
}

impl LoopHandles {
    fn lock_live_tasks(&self) -> MutexGuard<'_, HashMap<usize, Py<HandlerTask>>> {
        self.live_tasks
            .lock()
            .unwrap_or_else(|poisoned_lock| poisoned_lock.into_inner())
    }
}

/// Mutable half of a [`HandlerTask`]. Every transition happens on the task's
/// loop thread (steps and wakes run as loop callbacks; cross-thread
/// cancellation arrives via `call_soon_threadsafe`), so the mutex is for
/// `Sync` soundness, not contention.
#[derive(Default)]
struct TaskState {
    /// Built on the loop thread at the first step; `None` until then.
    coroutine: Option<Py<PyAny>>,
    /// Per-request `contextvars.Context`; every step runs inside it — the
    /// same semantics `asyncio.Task` provides.
    context: Option<Py<PyAny>>,
    /// The future the coroutine is currently suspended on, if any.
    waiting_on: Option<Py<PyAny>>,
    /// A protocol error to deliver on the next step (scheduled via
    /// `call_soon` so a misbehaving coroutine cannot starve the loop).
    deferred_throw: Option<Py<PyAny>>,
    /// A cancellation could not be absorbed by a pending future; deliver
    /// `CancelledError` at the next step instead of sending.
    must_cancel: bool,
    cancel_message: Option<Py<PyAny>>,
    /// Task-protocol `cancelling()` counter (`asyncio.timeout` and
    /// `TaskGroup` rely on cancel/uncancel bookkeeping).
    cancel_requests: u32,
    registered_with_asyncio: bool,
    name: Option<Py<PyAny>>,
    done_callbacks: Vec<(Py<PyAny>, Py<PyAny>)>,
    finished: bool,
    finished_cancelled: bool,
    cancelled_message: Option<Py<PyAny>>,
    stored_result: Option<Py<PyAny>>,
    stored_exception: Option<Py<PyAny>>,
}

/// What one advance of the coroutine produced.
enum StepOutcome<'py> {
    Returned(Bound<'py, PyAny>),
    Raised(PyErr),
    Yielded(Bound<'py, PyAny>),
}

/// How parking on a yielded object went.
enum ParkOutcome<'py> {
    Parked,
    /// The yield violated the future protocol; deliver this exception into
    /// the coroutine so the handler sees a specific error.
    BadYield(Bound<'py, PyAny>),
    Failed,
}

/// How the request ended, for the Future-protocol accessors.
enum CompletionKind {
    Returned(Py<PyAny>),
    Raised(Py<PyAny>),
    Cancelled(Option<Py<PyAny>>),
    /// Internal scheduler failure: no handler outcome exists.
    Aborted,
}

fn runtime_error_instance(py: Python<'_>, message: String) -> Bound<'_, PyAny> {
    PyRuntimeError::new_err(message)
        .value(py)
        .clone()
        .into_any()
}

fn cancelled_error_instance<'py>(py: Python<'py>, message: Option<Py<PyAny>>) -> Bound<'py, PyAny> {
    let cancelled = match message {
        Some(message) => CancelledError::new_err((message,)),
        None => CancelledError::new_err(()),
    };
    cancelled.value(py).clone().into_any()
}

/// One request's task-like: steps the handler coroutine to completion and
/// sends the response over the oneshot. Frozen: shared across threads
/// without the GIL. Weakref support is required by asyncio's task registry.
#[pyclass(
    frozen,
    weakref,
    freelist = 128,
    name = "HandlerTask",
    module = "rayo._core"
)]
pub struct HandlerTask {
    handler: Py<PyAny>,
    handler_kwargs: Py<PyDict>,
    loop_handles: Arc<LoopHandles>,
    channel: ResponseChannel,
    state: Mutex<TaskState>,
}

impl HandlerTask {
    fn lock_state(&self) -> MutexGuard<'_, TaskState> {
        self.state
            .lock()
            .unwrap_or_else(|poisoned_lock| poisoned_lock.into_inner())
    }

    /// Terminal bookkeeping, exactly once: record the outcome for the
    /// Future-protocol accessors, leave the registries, fire done callbacks,
    /// and release the response last so a woken client can never observe a
    /// half-finished task.
    fn finish(
        &self,
        py: Python<'_>,
        task_object: &Bound<'_, Self>,
        response: HandlerResponse,
        completion: CompletionKind,
    ) {
        let (was_registered, done_callbacks) = {
            let mut state = self.lock_state();
            if state.finished {
                self.channel.send(response); // no-op unless never sent
                return;
            }
            state.finished = true;
            state.waiting_on = None;
            state.deferred_throw = None;
            match completion {
                CompletionKind::Returned(return_value) => {
                    state.stored_result = Some(return_value);
                }
                CompletionKind::Raised(exception) => state.stored_exception = Some(exception),
                CompletionKind::Cancelled(message) => {
                    state.finished_cancelled = true;
                    state.cancelled_message = message;
                }
                CompletionKind::Aborted => {}
            }
            (
                std::mem::take(&mut state.registered_with_asyncio),
                std::mem::take(&mut state.done_callbacks),
            )
        };
        self.loop_handles
            .lock_live_tasks()
            .remove(&(task_object.as_ptr() as usize));
        if was_registered {
            if let Err(unregister_error) = self
                .loop_handles
                .unregister_task
                .bind(py)
                .call1((task_object,))
            {
                unregister_error.print(py);
            }
        }
        for (callback, callback_context) in done_callbacks {
            let kwargs = PyDict::new(py);
            let fired = kwargs
                .set_item(intern!(py, "context"), callback_context)
                .and_then(|()| {
                    self.loop_handles
                        .loop_object
                        .bind(py)
                        .call_method(
                            intern!(py, "call_soon"),
                            (callback, task_object),
                            Some(&kwargs),
                        )
                        .map(|_callback_handle| ())
                });
            if let Err(callback_error) = fired {
                callback_error.print(py);
            }
        }
        self.channel.send(response);
    }

    /// Close the coroutine so its `finally`/`__aexit__` cleanup runs, on the
    /// paths where it can never be stepped again. Cleanup that awaits cannot
    /// complete here; the failure is logged rather than silently skipped.
    fn close_coroutine_quietly(&self, py: Python<'_>) {
        let coroutine = {
            let state = self.lock_state();
            state
                .coroutine
                .as_ref()
                .map(|coroutine| coroutine.clone_ref(py))
        };
        if let Some(coroutine) = coroutine {
            if let Err(close_error) = coroutine.bind(py).call_method0(intern!(py, "close")) {
                close_error.print(py);
            }
        }
    }

    /// Terminal disposal for a task whose loop is gone: run what cleanup can
    /// still run, then finish as an internal error.
    fn dispose(&self, py: Python<'_>, task_object: &Bound<'_, Self>) {
        self.close_coroutine_quietly(py);
        self.finish(
            py,
            task_object,
            HandlerResponse::internal_error(),
            CompletionKind::Aborted,
        );
    }

    /// First step only: build the coroutine and per-request context on the
    /// loop thread, so decorator wrappers that run code before returning the
    /// coroutine (e.g. touching `asyncio.get_running_loop()`) see the same
    /// environment they would under asyncio. Returns `false` if the request
    /// was finished here due to a startup failure.
    fn start(&self, py: Python<'_>, task_object: &Bound<'_, Self>) -> bool {
        let coroutine = match self
            .handler
            .bind(py)
            .call((), Some(self.handler_kwargs.bind(py)))
        {
            Ok(coroutine) => coroutine,
            Err(factory_error) => {
                // Either the registered signature no longer matches (a Rayo
                // bug) or a decorator wrapper failed before returning the
                // coroutine (a handler bug); the traceback identifies which.
                let exception_value = factory_error.value(py).clone().into_any().unbind();
                let response = report_handler_error(py, factory_error);
                self.finish(
                    py,
                    task_object,
                    response,
                    CompletionKind::Raised(exception_value),
                );
                return false;
            }
        };
        // SAFETY: PyContext_CopyCurrent returns a new strong reference (or
        // null with an exception set); the thread is attached (`py`).
        let context_pointer = unsafe { ffi::PyContext_CopyCurrent() };
        if context_pointer.is_null() {
            PyErr::fetch(py).print(py);
            if let Err(close_error) = coroutine.call_method0(intern!(py, "close")) {
                close_error.print(py);
            }
            self.finish(
                py,
                task_object,
                HandlerResponse::internal_error(),
                CompletionKind::Aborted,
            );
            return false;
        }
        // SAFETY: the non-null return is a valid new strong reference.
        let context = unsafe { Bound::from_owned_ptr(py, context_pointer) };
        {
            let mut state = self.lock_state();
            state.coroutine = Some(coroutine.unbind());
            state.context = Some(context.unbind());
        }
        // Visibility in asyncio.all_tasks(); degraded introspection is not
        // worth failing the request over.
        match self
            .loop_handles
            .register_task
            .bind(py)
            .call1((task_object,))
        {
            Ok(_registered) => self.lock_state().registered_with_asyncio = true,
            Err(register_error) => register_error.print(py),
        }
        true
    }

    /// Advance the coroutine by sending `None`. `PyIter_Send` returns the
    /// coroutine's return value directly — no `StopIteration` is
    /// materialized on the happy path.
    fn send_step<'py>(&self, py: Python<'py>, coroutine: &Py<PyAny>) -> StepOutcome<'py> {
        let mut step_result: *mut ffi::PyObject = std::ptr::null_mut();
        // SAFETY: `coroutine` is a valid coroutine object kept alive by this
        // task, `Py_None()` is immortal, `step_result` is a valid out
        // pointer, and `py` witnesses that this thread is attached.
        let send_status =
            unsafe { ffi::PyIter_Send(coroutine.as_ptr(), ffi::Py_None(), &mut step_result) };
        match send_status {
            ffi::PySendResult::PYGEN_RETURN => {
                // SAFETY: PYGEN_RETURN hands us a new strong reference to the
                // coroutine's return value.
                StepOutcome::Returned(unsafe { Bound::from_owned_ptr(py, step_result) })
            }
            ffi::PySendResult::PYGEN_NEXT => {
                // SAFETY: PYGEN_NEXT hands us a new strong reference to the
                // yielded object.
                StepOutcome::Yielded(unsafe { Bound::from_owned_ptr(py, step_result) })
            }
            ffi::PySendResult::PYGEN_ERROR => StepOutcome::Raised(PyErr::fetch(py)),
        }
    }

    /// Advance the coroutine by throwing `exception` into it.
    fn throw_step<'py>(
        &self,
        py: Python<'py>,
        coroutine: &Py<PyAny>,
        exception: Bound<'py, PyAny>,
    ) -> StepOutcome<'py> {
        match coroutine
            .bind(py)
            .call_method1(intern!(py, "throw"), (exception,))
        {
            Ok(yielded) => StepOutcome::Yielded(yielded),
            Err(step_error) => {
                if step_error.is_instance_of::<PyStopIteration>(py) {
                    // The coroutine caught the exception and returned
                    // normally; the return value rides the StopIteration.
                    match step_error.value(py).getattr(intern!(py, "value")) {
                        Ok(return_value) => StepOutcome::Returned(return_value),
                        Err(extraction_error) => StepOutcome::Raised(extraction_error),
                    }
                } else {
                    StepOutcome::Raised(step_error)
                }
            }
        }
    }

    /// Park the coroutine on a yielded future, following the
    /// `_asyncio_future_blocking` protocol exactly as `asyncio.Task` does.
    fn park_on_future<'py>(
        &self,
        py: Python<'py>,
        task_object: &Bound<'_, Self>,
        yielded: &Bound<'py, PyAny>,
    ) -> ParkOutcome<'py> {
        let blocking_attribute = intern!(py, "_asyncio_future_blocking");
        let Ok(blocking_flag) = yielded.getattr(blocking_attribute) else {
            return ParkOutcome::BadYield(runtime_error_instance(
                py,
                format!(
                    "handler awaited an object Rayo's scheduler does not understand \
                     (type {}); only asyncio futures and things built on them can be awaited",
                    yielded
                        .get_type()
                        .name()
                        .map(|type_name| type_name.to_string())
                        .unwrap_or_else(|_| String::from("<unknown>")),
                ),
            ));
        };
        if !blocking_flag.is_truthy().unwrap_or(false) {
            return ParkOutcome::BadYield(runtime_error_instance(
                py,
                String::from(
                    "handler coroutine used `yield` where `await` was expected \
                     (a future was passed through without its blocking flag set)",
                ),
            ));
        }
        // Cross-loop awaits are an error, exactly as under asyncio.Task:
        // Rayo runs one loop per core, and a loop-bound object awaited from
        // another loop's request would wake on the wrong thread — or never.
        if let Ok(future_loop) = yielded.call_method0(intern!(py, "get_loop")) {
            if future_loop.as_ptr() != self.loop_handles.loop_object.as_ptr() {
                return ParkOutcome::BadYield(runtime_error_instance(
                    py,
                    String::from(
                        "handler awaited a future attached to a different event loop; \
                         Rayo runs one event loop per core, so loop-bound objects \
                         (futures, locks, queues) created on one loop cannot be awaited \
                         from a request running on another — create them inside the \
                         handler, or pin them to a single loop",
                    ),
                ));
            }
        }
        if let Err(flag_error) = yielded.setattr(blocking_attribute, false) {
            flag_error.print(py);
            return ParkOutcome::Failed;
        }
        {
            let mut state = self.lock_state();
            state.waiting_on = Some(yielded.clone().unbind());
        }
        if let Err(callback_error) =
            yielded.call_method1(intern!(py, "add_done_callback"), (task_object,))
        {
            callback_error.print(py);
            self.lock_state().waiting_on = None;
            return ParkOutcome::Failed;
        }
        ParkOutcome::Parked
    }

    /// Advance the coroutine once and dispose of the outcome: finish on
    /// return/raise, park on a yielded future, or reschedule through the
    /// loop (bare yields and protocol errors both defer via `call_soon`, as
    /// Task does, so a misbehaving coroutine cannot starve the loop thread).
    /// Runs inside the entered context and task registration.
    fn step_once<'py>(
        &self,
        py: Python<'py>,
        task_object: &Bound<'_, Self>,
        coroutine: &Py<PyAny>,
        pending_throw: Option<Bound<'py, PyAny>>,
    ) {
        let outcome = match pending_throw {
            Some(exception) => self.throw_step(py, coroutine, exception),
            None => self.send_step(py, coroutine),
        };
        match outcome {
            StepOutcome::Returned(return_value) => {
                let response = response_from_return_value(&return_value);
                self.finish(
                    py,
                    task_object,
                    response,
                    CompletionKind::Returned(return_value.unbind()),
                );
            }
            StepOutcome::Raised(handler_error) => {
                if handler_error.is_instance_of::<CancelledError>(py) {
                    // A cancelled request ends quietly: the client is gone,
                    // or a timeout already surfaced the outcome to the
                    // handler. Matches asyncio's no-log policy for cancelled
                    // tasks.
                    let cancellation_message = handler_error
                        .value(py)
                        .getattr(intern!(py, "args"))
                        .ok()
                        .and_then(|arguments| arguments.get_item(0).ok())
                        .map(Bound::unbind);
                    self.finish(
                        py,
                        task_object,
                        HandlerResponse::internal_error(),
                        CompletionKind::Cancelled(cancellation_message),
                    );
                } else {
                    let exception_value = handler_error.value(py).clone().into_any().unbind();
                    let response = report_handler_error(py, handler_error);
                    self.finish(
                        py,
                        task_object,
                        response,
                        CompletionKind::Raised(exception_value),
                    );
                }
            }
            StepOutcome::Yielded(yielded) => {
                if yielded.is_none() {
                    // Bare `yield` (asyncio.sleep(0)): take one trip through
                    // the loop, then resume.
                    self.reschedule_or_abort(py, task_object);
                    return;
                }
                match self.park_on_future(py, task_object, &yielded) {
                    ParkOutcome::Parked => self.apply_cancel_requested_mid_step(py),
                    ParkOutcome::BadYield(protocol_error) => {
                        self.lock_state().deferred_throw = Some(protocol_error.unbind());
                        self.reschedule_or_abort(py, task_object);
                    }
                    ParkOutcome::Failed => {
                        self.close_coroutine_quietly(py);
                        self.finish(
                            py,
                            task_object,
                            HandlerResponse::internal_error(),
                            CompletionKind::Aborted,
                        );
                    }
                }
            }
        }
    }

    /// Queue the next turn via `call_soon`; if even that fails the loop is
    /// unusable, so run what cleanup can run and resolve the request.
    fn reschedule_or_abort(&self, py: Python<'_>, task_object: &Bound<'_, Self>) {
        if let Err(reschedule_error) = self
            .loop_handles
            .loop_object
            .bind(py)
            .call_method1(intern!(py, "call_soon"), (task_object,))
        {
            reschedule_error.print(py);
            self.close_coroutine_quietly(py);
            self.finish(
                py,
                task_object,
                HandlerResponse::internal_error(),
                CompletionKind::Aborted,
            );
        }
    }

    /// Task.__step parity: a `cancel()` that lands while the coroutine is
    /// mid-step finds no parked future to absorb it and sets `must_cancel`;
    /// re-apply it against the future the step just parked on.
    fn apply_cancel_requested_mid_step(&self, py: Python<'_>) {
        let pending_cancel = {
            let state = self.lock_state();
            if state.must_cancel {
                state.waiting_on.as_ref().map(|future| {
                    (
                        future.clone_ref(py),
                        state
                            .cancel_message
                            .as_ref()
                            .map(|message| message.clone_ref(py)),
                    )
                })
            } else {
                None
            }
        };
        let Some((parked_future, cancel_message)) = pending_cancel else {
            return;
        };
        match parked_future
            .bind(py)
            .call_method1(intern!(py, "cancel"), (cancel_message,))
        {
            Ok(cancel_result) => {
                if cancel_result.is_truthy().unwrap_or(false) {
                    // The future delivers the CancelledError through the
                    // normal wake path; the deferred flag is spent.
                    self.lock_state().must_cancel = false;
                }
            }
            Err(cancel_error) => cancel_error.print(py),
        }
    }
}

#[pymethods]
impl HandlerTask {
    /// One scheduler turn. Invoked with no argument by `call_soon` /
    /// `call_soon_threadsafe` (initial dispatch, bare-yield resume, deferred
    /// throws) and with the finished future when running as a done callback.
    #[pyo3(signature = (finished_future = None))]
    fn __call__(task_object: &Bound<'_, Self>, finished_future: Option<&Bound<'_, PyAny>>) {
        let py = task_object.py();
        let task = task_object.get();

        let must_cancel;
        let cancel_message;
        let deferred_throw;
        let needs_start;
        {
            let mut state = task.lock_state();
            if state.finished {
                return; // spurious wake after completion
            }
            state.waiting_on = None;
            must_cancel = state.must_cancel;
            state.must_cancel = false;
            cancel_message = state.cancel_message.take();
            deferred_throw = state.deferred_throw.take();
            needs_start = state.coroutine.is_none();
        }

        if needs_start {
            if must_cancel {
                // Cancelled before the first step: the coroutine is never
                // built, so there is nothing to unwind.
                task.finish(
                    py,
                    task_object,
                    HandlerResponse::internal_error(),
                    CompletionKind::Cancelled(cancel_message),
                );
                return;
            }
            if !task.start(py, task_object) {
                return;
            }
        }

        // A wake from a finished future may carry an exception to deliver;
        // deferred protocol errors and pending cancellations override it.
        let mut pending_throw: Option<Bound<'_, PyAny>> =
            deferred_throw.map(|exception| exception.into_bound(py));
        if let Some(future) = finished_future {
            if let Err(wake_error) = future.call_method0(intern!(py, "result")) {
                pending_throw = Some(wake_error.value(py).clone().into_any());
            }
        }
        if must_cancel {
            let already_cancelled = pending_throw
                .as_ref()
                .is_some_and(|exception| exception.is_instance_of::<CancelledError>());
            if !already_cancelled {
                pending_throw = Some(cancelled_error_instance(py, cancel_message));
            }
        }

        let (coroutine, context) = {
            let state = task.lock_state();
            let coroutine = state
                .coroutine
                .as_ref()
                .map(|coroutine| coroutine.clone_ref(py));
            let context = state.context.as_ref().map(|context| context.clone_ref(py));
            match (coroutine, context) {
                (Some(coroutine), Some(context)) => (coroutine, context),
                // Unreachable after a successful start; be defensive.
                _ => return,
            }
        };

        if let Err(enter_error) = task
            .loop_handles
            .enter_task
            .bind(py)
            .call1((&task.loop_handles.loop_object, task_object))
        {
            enter_error.print(py);
            task.close_coroutine_quietly(py);
            task.finish(
                py,
                task_object,
                HandlerResponse::internal_error(),
                CompletionKind::Aborted,
            );
            return;
        }
        // SAFETY: `context` is a valid contextvars.Context this task owns;
        // the matching PyContext_Exit below runs before this function
        // returns, and the context is only entered by this loop thread.
        if unsafe { ffi::PyContext_Enter(context.as_ptr()) } != 0 {
            PyErr::fetch(py).print(py);
            let _ = task
                .loop_handles
                .leave_task
                .bind(py)
                .call1((&task.loop_handles.loop_object, task_object));
            task.close_coroutine_quietly(py);
            task.finish(
                py,
                task_object,
                HandlerResponse::internal_error(),
                CompletionKind::Aborted,
            );
            return;
        }

        task.step_once(py, task_object, &coroutine, pending_throw);

        // SAFETY: balances the successful PyContext_Enter above.
        if unsafe { ffi::PyContext_Exit(context.as_ptr()) } != 0 {
            PyErr::fetch(py).print(py);
        }
        if let Err(leave_error) = task
            .loop_handles
            .leave_task
            .bind(py)
            .call1((&task.loop_handles.loop_object, task_object))
        {
            leave_error.print(py);
        }
    }

    /// Task-protocol cancellation: absorbed by the awaited future when
    /// possible, otherwise delivered as `CancelledError` at the next step.
    #[pyo3(signature = (msg = None))]
    fn cancel(&self, py: Python<'_>, msg: Option<Bound<'_, PyAny>>) -> bool {
        let waiting_on = {
            let mut state = self.lock_state();
            if state.finished {
                return false;
            }
            state.cancel_requests += 1;
            state.cancel_message = msg.as_ref().map(|message| message.clone().unbind());
            state.waiting_on.as_ref().map(|future| future.clone_ref(py))
        };
        if let Some(future) = waiting_on {
            match future.bind(py).call_method1(intern!(py, "cancel"), (msg,)) {
                Ok(cancel_result) => {
                    if cancel_result.is_truthy().unwrap_or(false) {
                        // The future delivers the CancelledError through the
                        // normal wake path.
                        return true;
                    }
                }
                Err(cancel_error) => cancel_error.print(py),
            }
        }
        // No future could absorb it (none pending, or already finishing):
        // the next scheduled step throws instead of sending.
        let mut state = self.lock_state();
        if state.finished {
            return false;
        }
        state.must_cancel = true;
        true
    }

    /// Task-protocol `uncancel`, used by `asyncio.timeout` and `TaskGroup`.
    fn uncancel(&self) -> u32 {
        let mut state = self.lock_state();
        state.cancel_requests = state.cancel_requests.saturating_sub(1);
        if state.cancel_requests == 0 {
            state.must_cancel = false;
        }
        state.cancel_requests
    }

    fn cancelling(&self) -> u32 {
        self.lock_state().cancel_requests
    }

    fn done(&self) -> bool {
        self.lock_state().finished
    }

    fn cancelled(&self) -> bool {
        let state = self.lock_state();
        state.finished && state.finished_cancelled
    }

    fn get_loop(&self, py: Python<'_>) -> Py<PyAny> {
        self.loop_handles.loop_object.clone_ref(py)
    }

    fn get_coro(&self, py: Python<'_>) -> Option<Py<PyAny>> {
        self.lock_state()
            .coroutine
            .as_ref()
            .map(|coroutine| coroutine.clone_ref(py))
    }

    fn get_context(&self, py: Python<'_>) -> Option<Py<PyAny>> {
        self.lock_state()
            .context
            .as_ref()
            .map(|context| context.clone_ref(py))
    }

    fn get_name(&self, py: Python<'_>) -> Py<PyAny> {
        match self.lock_state().name.as_ref() {
            Some(name) => name.clone_ref(py),
            None => intern!(py, "rayo-handler").clone().into_any().unbind(),
        }
    }

    fn set_name(&self, value: Bound<'_, PyAny>) -> PyResult<()> {
        let name = value.str()?;
        self.lock_state().name = Some(name.into_any().unbind());
        Ok(())
    }

    /// Future-protocol result: the handler's return value once finished.
    fn result(&self, py: Python<'_>) -> PyResult<Py<PyAny>> {
        let state = self.lock_state();
        if !state.finished {
            return Err(InvalidStateError::new_err(
                "this request's handler is still running",
            ));
        }
        if state.finished_cancelled {
            return Err(match state.cancelled_message.as_ref() {
                Some(message) => CancelledError::new_err((message.clone_ref(py),)),
                None => CancelledError::new_err(()),
            });
        }
        if let Some(exception) = state.stored_exception.as_ref() {
            return Err(PyErr::from_value(exception.bind(py).clone()));
        }
        Ok(match state.stored_result.as_ref() {
            Some(return_value) => return_value.clone_ref(py),
            None => py.None(),
        })
    }

    /// Future-protocol exception accessor.
    fn exception(&self, py: Python<'_>) -> PyResult<Option<Py<PyAny>>> {
        let state = self.lock_state();
        if !state.finished {
            return Err(InvalidStateError::new_err(
                "this request's handler is still running",
            ));
        }
        if state.finished_cancelled {
            return Err(match state.cancelled_message.as_ref() {
                Some(message) => CancelledError::new_err((message.clone_ref(py),)),
                None => CancelledError::new_err(()),
            });
        }
        Ok(state
            .stored_exception
            .as_ref()
            .map(|exception| exception.clone_ref(py)))
    }

    /// Future-protocol done callbacks (used by tracing/APM integrations via
    /// `asyncio.current_task().add_done_callback(...)`).
    #[pyo3(signature = (callback, *, context = None))]
    fn add_done_callback(
        task_object: &Bound<'_, Self>,
        callback: Bound<'_, PyAny>,
        context: Option<Bound<'_, PyAny>>,
    ) -> PyResult<()> {
        let py = task_object.py();
        let task = task_object.get();
        let callback_context = match context {
            Some(explicit_context) => explicit_context,
            None => {
                // SAFETY: returns a new strong reference (or null with an
                // exception set); the thread is attached.
                let context_pointer = unsafe { ffi::PyContext_CopyCurrent() };
                if context_pointer.is_null() {
                    return Err(PyErr::fetch(py));
                }
                // SAFETY: the non-null return is a valid new strong reference.
                unsafe { Bound::from_owned_ptr(py, context_pointer) }
            }
        };
        let fire_immediately = {
            let mut state = task.lock_state();
            if state.finished {
                true
            } else {
                state
                    .done_callbacks
                    .push((callback.clone().unbind(), callback_context.clone().unbind()));
                false
            }
        };
        if fire_immediately {
            let kwargs = PyDict::new(py);
            kwargs.set_item(intern!(py, "context"), callback_context)?;
            task.loop_handles.loop_object.bind(py).call_method(
                intern!(py, "call_soon"),
                (callback, task_object),
                Some(&kwargs),
            )?;
        }
        Ok(())
    }

    fn remove_done_callback(&self, py: Python<'_>, callback: Bound<'_, PyAny>) -> usize {
        let mut state = self.lock_state();
        let callbacks_before = state.done_callbacks.len();
        state
            .done_callbacks
            .retain(|(existing_callback, _context)| {
                !existing_callback.bind(py).eq(&callback).unwrap_or(false)
            });
        callbacks_before - state.done_callbacks.len()
    }

    /// Keep the task ↔ future cycle a parked request forms visible to the
    /// cycle GC. `loop_handles` is shared through an `Arc` and deliberately
    /// not visited (see [`LoopHandles`]); skipping fields on a contended
    /// state lock only delays collection, never breaks it.
    fn __traverse__(&self, visit: PyVisit<'_>) -> Result<(), PyTraverseError> {
        visit.call(&self.handler)?;
        visit.call(&self.handler_kwargs)?;
        if let Ok(state) = self.state.try_lock() {
            visit.call(&state.coroutine)?;
            visit.call(&state.context)?;
            visit.call(&state.waiting_on)?;
            visit.call(&state.deferred_throw)?;
            visit.call(&state.cancel_message)?;
            visit.call(&state.cancelled_message)?;
            visit.call(&state.name)?;
            visit.call(&state.stored_result)?;
            visit.call(&state.stored_exception)?;
            for (callback, callback_context) in &state.done_callbacks {
                visit.call(callback)?;
                visit.call(callback_context)?;
            }
        }
        Ok(())
    }
}

/// The Python event-loop thread and the handles needed to schedule onto it.
pub struct EventLoop {
    handles: Arc<LoopHandles>,
    thread: Mutex<Option<JoinHandle<()>>>,
    /// Requests scheduled but not yet completed — the load signal for
    /// least-loaded placement, and an ops gauge. Heuristic: `Relaxed`
    /// everywhere, exactness is not required for either use.
    in_flight: Arc<AtomicUsize>,
}

impl EventLoop {
    /// Create an asyncio event loop and run it forever on a dedicated thread.
    pub fn start(py: Python<'_>, thread_name: String) -> PyResult<Self> {
        let asyncio = py.import("asyncio")?;
        let loop_object = asyncio.call_method0("new_event_loop")?;
        let tasks_module = asyncio.getattr("tasks")?;
        let task_hook = |hook_name: &str| {
            tasks_module.getattr(hook_name).map_err(|attribute_error| {
                PyRuntimeError::new_err(format!(
                    "Rayo's scheduler integrates with asyncio through \
                     asyncio.tasks.{hook_name} (one of CPython's task-extension \
                     hooks, present since 3.7), which this Python build does not \
                     provide: {attribute_error}"
                ))
            })
        };
        let enter_task = task_hook("_enter_task")?;
        let leave_task = task_hook("_leave_task")?;
        let register_task = task_hook("_register_task")?;
        let unregister_task = task_hook("_unregister_task")?;

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
            handles: Arc::new(LoopHandles {
                loop_object: loop_object.unbind(),
                enter_task: enter_task.unbind(),
                leave_task: leave_task.unbind(),
                register_task: register_task.unbind(),
                unregister_task: unregister_task.unbind(),
                live_tasks: Mutex::new(HashMap::new()),
            }),
            thread: Mutex::new(Some(thread)),
            in_flight: Arc::new(AtomicUsize::new(0)),
        })
    }

    pub fn in_flight_count(&self) -> usize {
        self.in_flight.load(Ordering::Relaxed)
    }

    /// Create the request's task and schedule its first step onto the event
    /// loop. Always resolves the returned request — dispatch failures become
    /// 500s, never hangs.
    pub fn schedule(
        &self,
        py: Python<'_>,
        handler: &Py<PyAny>,
        handler_kwargs: Bound<'_, PyDict>,
    ) -> DispatchedRequest {
        let (response_sender, response_receiver) = oneshot::channel();
        self.in_flight.fetch_add(1, Ordering::Relaxed);
        let channel = ResponseChannel {
            response_sender: Mutex::new(Some(response_sender)),
            in_flight: Arc::clone(&self.in_flight),
        };

        let task = match Py::new(
            py,
            HandlerTask {
                handler: handler.clone_ref(py),
                handler_kwargs: handler_kwargs.unbind(),
                loop_handles: Arc::clone(&self.handles),
                channel,
                state: Mutex::new(TaskState::default()),
            },
        ) {
            Ok(task) => task,
            Err(allocation_error) => {
                allocation_error.print(py);
                // The channel went down with the failed task: its Drop
                // balanced the gauge, and the dropped sender surfaces as a
                // 500 on the receiver side.
                return DispatchedRequest::failed(response_receiver);
            }
        };

        // Registered before the first step can possibly run, so the finish
        // path always finds (and removes) the entry.
        self.handles
            .lock_live_tasks()
            .insert(task.as_ptr() as usize, task.clone_ref(py));

        let scheduled = self
            .handles
            .loop_object
            .bind(py)
            .call_method1(intern!(py, "call_soon_threadsafe"), (&task,));
        if let Err(scheduling_error) = scheduled {
            scheduling_error.print(py);
            self.handles
                .lock_live_tasks()
                .remove(&(task.as_ptr() as usize));
            // The coroutine is built on the first step, which will never
            // run — nothing to close.
            task.get().channel.send(HandlerResponse::internal_error());
            return DispatchedRequest::failed(response_receiver);
        }
        DispatchedRequest {
            response_receiver,
            task: Some(task),
        }
    }

    /// Schedule cancellation of every live task on this loop (graceful
    /// shutdown: handlers unwind, cleanup runs, responses resolve).
    pub fn cancel_in_flight(&self, py: Python<'_>) {
        let live_tasks: Vec<Py<HandlerTask>> = {
            let registry = self.handles.lock_live_tasks();
            registry.values().map(|task| task.clone_ref(py)).collect()
        };
        for task in live_tasks {
            let scheduled =
                task.bind(py)
                    .getattr(intern!(py, "cancel"))
                    .and_then(|cancel_method| {
                        self.handles
                            .loop_object
                            .bind(py)
                            .call_method1(intern!(py, "call_soon_threadsafe"), (cancel_method,))
                    });
            // A loop that refuses the callback is closing; stop() disposes
            // of whatever remains.
            let _closing = scheduled;
        }
    }

    /// Stop the loop, join its thread, and dispose of any task that can
    /// never run again. Idempotent.
    pub fn stop(&self, py: Python<'_>) {
        let thread_handle = {
            let mut thread_slot = self
                .thread
                .lock()
                .unwrap_or_else(|poisoned_lock| poisoned_lock.into_inner());
            thread_slot.take()
        };
        // Only the call that wins the join handle stops the loop; later
        // calls would just be poking a closed loop.
        if let Some(handle) = thread_handle {
            let event_loop = self.handles.loop_object.bind(py);
            if let Ok(stop_method) = event_loop.getattr("stop") {
                if let Err(stop_error) =
                    event_loop.call_method1("call_soon_threadsafe", (stop_method,))
                {
                    stop_error.print(py);
                }
            }
            // Detach so the loop thread can re-attach to finish shutting down.
            py.detach(|| {
                if handle.join().is_err() {
                    eprintln!("rayo-dispatch: event-loop thread panicked during shutdown");
                }
            });
        }
        // Anything still registered after the loop stopped can never be
        // stepped again: run what cleanup can run and resolve its response,
        // instead of leaking the coroutine and pinning the gauge.
        let leftover_tasks: Vec<Py<HandlerTask>> = {
            let mut registry = self.handles.lock_live_tasks();
            registry.drain().map(|(_pointer, task)| task).collect()
        };
        for task in leftover_tasks {
            let bound_task = task.bind(py);
            bound_task.get().dispose(py, bound_task);
        }
    }
}

/// A dispatched request as seen by the server task: await [`response`] for
/// the outcome. Dropping it before the response arrives cancels the handler —
/// this is how client disconnects propagate (Hyper drops the response future
/// when the connection dies).
///
/// [`response`]: DispatchedRequest::response
pub struct DispatchedRequest {
    response_receiver: oneshot::Receiver<HandlerResponse>,
    task: Option<Py<HandlerTask>>,
}

impl DispatchedRequest {
    /// Dispatch never reached the loop; the outcome (or the dropped sender)
    /// is already in the channel and there is nothing left to cancel.
    fn failed(response_receiver: oneshot::Receiver<HandlerResponse>) -> Self {
        Self {
            response_receiver,
            task: None,
        }
    }

    /// Wait for the handler's response. `None` means the scheduler went away
    /// without responding (the event loop shut down mid-request).
    pub async fn response(&mut self) -> Option<HandlerResponse> {
        let outcome = (&mut self.response_receiver).await;
        self.task = None;
        outcome.ok()
    }
}

impl Drop for DispatchedRequest {
    fn drop(&mut self) {
        let Some(task) = self.task.take() else {
            return;
        };
        if task.get().lock_state().finished {
            return;
        }
        // The response future was dropped before completion — the client
        // disconnected. Propagate as cancellation onto the loop thread. A
        // failure here means the loop is closing; stop() disposes of the
        // task instead.
        Python::attach(|py| {
            let _scheduled_or_shutting_down = task
                .bind(py)
                .getattr(intern!(py, "cancel"))
                .and_then(|cancel_method| {
                    task.get()
                        .loop_handles
                        .loop_object
                        .bind(py)
                        .call_method1(intern!(py, "call_soon_threadsafe"), (cancel_method,))
                });
        });
    }
}

/// N event loops on N threads, one intended per core on free-threaded builds
/// (where they run Python truly in parallel) and one total on GIL builds.
/// Requests go to the least-loaded loop (in-flight count, scan started at a
/// rotating offset so ties spread instead of piling onto loop 0). The pool is
/// `Sync` and lock-free on the scheduling path (invariant 4).
pub struct EventLoopPool {
    loops: Vec<EventLoop>,
    scan_start_index: AtomicUsize,
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
            scan_start_index: AtomicUsize::new(0),
        })
    }

    pub fn size(&self) -> usize {
        self.loops.len()
    }

    /// Requests scheduled but not yet completed, across all loops.
    pub fn total_in_flight(&self) -> usize {
        self.loops.iter().map(EventLoop::in_flight_count).sum()
    }

    /// Schedule onto the least-loaded loop.
    pub fn schedule(
        &self,
        py: Python<'_>,
        handler: &Py<PyAny>,
        handler_kwargs: Bound<'_, PyDict>,
    ) -> DispatchedRequest {
        let loop_count = self.loops.len();
        let scan_start = self.scan_start_index.fetch_add(1, Ordering::Relaxed);
        let mut chosen_index = scan_start % loop_count;
        let mut lowest_load = self.loops[chosen_index].in_flight_count();
        for scan_offset in 1..loop_count {
            if lowest_load == 0 {
                break; // an idle loop cannot be beaten
            }
            let candidate_index = (scan_start + scan_offset) % loop_count;
            let candidate_load = self.loops[candidate_index].in_flight_count();
            if candidate_load < lowest_load {
                chosen_index = candidate_index;
                lowest_load = candidate_load;
            }
        }
        self.loops[chosen_index].schedule(py, handler, handler_kwargs)
    }

    /// Schedule cancellation of every live task on every loop.
    pub fn cancel_in_flight(&self, py: Python<'_>) {
        for event_loop in &self.loops {
            event_loop.cancel_in_flight(py);
        }
    }

    /// Stop every loop, join its thread, and dispose of stranded tasks.
    /// Idempotent.
    pub fn stop(&self, py: Python<'_>) {
        for event_loop in &self.loops {
            event_loop.stop(py);
        }
    }
}

/// Run a sync handler to completion on the current thread (the caller is
/// responsible for putting this on a blocking-capable thread).
///
/// Cancellation semantics, documented user-facing behavior: a sync handler
/// cannot be cancelled mid-run — there is no suspension point to deliver
/// `CancelledError` to. On client disconnect it finishes anyway and the
/// response is discarded (the response channel's receiver is gone). Handlers
/// that need cooperative cancellation must be `async`.
pub fn execute_sync_handler(
    handler: &Bound<'_, PyAny>,
    handler_kwargs: &Bound<'_, PyDict>,
) -> HandlerResponse {
    match handler.call((), Some(handler_kwargs)) {
        Ok(return_value) => response_from_return_value(&return_value),
        Err(handler_error) => report_handler_error(handler.py(), handler_error),
    }
}
