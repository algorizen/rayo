//! `rayo-dispatch` — the Rust↔Python dispatch boundary. See ADR-0004 and
//! docs/design/dispatch-scheduler.md.
//!
//! Async handlers are driven by [`HandlerTask`], a frozen task-like pyclass
//! that steps the handler coroutine directly with `PyIter_Send` — no
//! `asyncio.Task` is created per request. Yielded futures get the task as a
//! done callback; bare yields take one trip through the loop via `call_soon`;
//! cross-thread entry (initial dispatch, disconnect cancellation) is the only
//! use of `call_soon_threadsafe`. Every step runs inside a per-request
//! `contextvars.Context` and between `_enter_task`/`_leave_task`, so
//! `asyncio.current_task()`, `asyncio.timeout`, and `TaskGroup` behave as
//! they would under `asyncio.Task`. The design is ported from Granian's
//! MIT-licensed scheduler (credited in ADR-0004).
//!
//! Boundary invariants: the request path enters Python exactly once, and
//! responses are serialized to bytes in Rust while reading (never creating)
//! Python objects.
//!
//! Per code standards, this crate is where the project's `unsafe` FFI code is
//! confined; every `unsafe` block carries a `// SAFETY:` comment.

use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex, MutexGuard};
use std::thread::JoinHandle;

use pyo3::exceptions::asyncio::CancelledError;
use pyo3::exceptions::{PyRuntimeError, PyStopIteration};
use pyo3::ffi;
use pyo3::intern;
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

/// The Rust half of a request's completion: the oneshot back to the server
/// task plus the owning loop's in-flight gauge. `send` fires at most once;
/// if the channel is dropped without ever sending (the loop shut down, an
/// allocation failed), `Drop` still balances the gauge and the dropped
/// sender surfaces as a 500 on the receiver side.
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

/// Mutable half of a [`HandlerTask`]. Every transition happens on the task's
/// loop thread (steps and wakes run as loop callbacks; cross-thread
/// cancellation arrives via `call_soon_threadsafe`), so the mutex is for
/// `Sync` soundness, not contention.
#[derive(Default)]
struct HandlerTaskState {
    /// The future the coroutine is currently suspended on, if any.
    waiting_on: Option<Py<PyAny>>,
    /// A cancellation could not be absorbed by a pending future; deliver
    /// `CancelledError` at the next step instead of sending.
    must_cancel: bool,
    cancel_message: Option<Py<PyAny>>,
    /// Task-protocol `cancelling()` counter (`asyncio.timeout` and
    /// `TaskGroup` rely on cancel/uncancel bookkeeping).
    cancel_requests: u32,
    finished: bool,
    finished_cancelled: bool,
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
/// without the GIL. Freelisted: allocated and dropped once per request.
#[pyclass(frozen, freelist = 128, name = "HandlerTask", module = "rayo._core")]
pub struct HandlerTask {
    coroutine: Py<PyAny>,
    /// Per-request `contextvars.Context`, copied at dispatch. Every step runs
    /// inside it — the same semantics `asyncio.Task` provides.
    context: Py<PyAny>,
    event_loop: Py<PyAny>,
    enter_task: Py<PyAny>,
    leave_task: Py<PyAny>,
    channel: ResponseChannel,
    state: Mutex<HandlerTaskState>,
}

impl HandlerTask {
    fn lock_state(&self) -> MutexGuard<'_, HandlerTaskState> {
        self.state
            .lock()
            .unwrap_or_else(|poisoned_lock| poisoned_lock.into_inner())
    }

    fn finish(&self, response: HandlerResponse, cancelled: bool) {
        {
            let mut state = self.lock_state();
            state.finished = true;
            state.finished_cancelled = cancelled;
            state.waiting_on = None;
        }
        self.channel.send(response);
    }

    /// Advance the coroutine by sending `None`. `PyIter_Send` returns the
    /// coroutine's return value directly — no `StopIteration` is
    /// materialized on the happy path.
    fn send_step<'py>(&self, py: Python<'py>) -> StepOutcome<'py> {
        let mut step_result: *mut ffi::PyObject = std::ptr::null_mut();
        // SAFETY: `coroutine` is a valid coroutine object kept alive by this
        // task, `Py_None()` is immortal, `step_result` is a valid out
        // pointer, and `py` witnesses that this thread is attached.
        let send_status =
            unsafe { ffi::PyIter_Send(self.coroutine.as_ptr(), ffi::Py_None(), &mut step_result) };
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
    fn throw_step<'py>(&self, py: Python<'py>, exception: Bound<'py, PyAny>) -> StepOutcome<'py> {
        match self
            .coroutine
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

    /// Step repeatedly until the coroutine parks on a future, reschedules,
    /// or finishes. Runs inside the entered context and task registration.
    fn step_until_suspended<'py>(
        &self,
        py: Python<'py>,
        task_object: &Bound<'_, Self>,
        mut pending_throw: Option<Bound<'py, PyAny>>,
    ) {
        loop {
            let outcome = match pending_throw.take() {
                Some(exception) => self.throw_step(py, exception),
                None => self.send_step(py),
            };
            match outcome {
                StepOutcome::Returned(return_value) => {
                    self.finish(response_from_return_value(&return_value), false);
                    return;
                }
                StepOutcome::Raised(handler_error) => {
                    if handler_error.is_instance_of::<CancelledError>(py) {
                        // A cancelled request ends quietly: the client is
                        // gone, or a timeout already surfaced the outcome to
                        // the handler. Matches asyncio's no-log policy for
                        // cancelled tasks.
                        self.finish(HandlerResponse::internal_error(), true);
                    } else {
                        self.finish(report_handler_error(py, handler_error), false);
                    }
                    return;
                }
                StepOutcome::Yielded(yielded) => {
                    if yielded.is_none() {
                        // Bare `yield` (asyncio.sleep(0)): take one trip
                        // through the loop, then resume.
                        if let Err(reschedule_error) = self
                            .event_loop
                            .bind(py)
                            .call_method1(intern!(py, "call_soon"), (task_object,))
                        {
                            reschedule_error.print(py);
                            self.finish(HandlerResponse::internal_error(), false);
                        }
                        return;
                    }
                    match self.park_on_future(py, task_object, &yielded) {
                        ParkOutcome::Parked => return,
                        ParkOutcome::BadYield(protocol_error) => {
                            pending_throw = Some(protocol_error);
                        }
                        ParkOutcome::Failed => {
                            self.finish(HandlerResponse::internal_error(), false);
                            return;
                        }
                    }
                }
            }
        }
    }
}

#[pymethods]
impl HandlerTask {
    /// One scheduler turn. Invoked with no argument by `call_soon` /
    /// `call_soon_threadsafe` (initial dispatch, bare-yield resume) and with
    /// the finished future when running as a done callback.
    #[pyo3(signature = (finished_future = None))]
    fn __call__(task_object: &Bound<'_, Self>, finished_future: Option<&Bound<'_, PyAny>>) {
        let py = task_object.py();
        let task = task_object.get();

        let must_cancel;
        let cancel_message;
        {
            let mut state = task.lock_state();
            if state.finished {
                return; // spurious wake after completion
            }
            state.waiting_on = None;
            must_cancel = state.must_cancel;
            state.must_cancel = false;
            cancel_message = state.cancel_message.take();
        }

        // A wake from a finished future may carry an exception to deliver.
        let mut pending_throw: Option<Bound<'_, PyAny>> = None;
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

        if let Err(enter_error) = task
            .enter_task
            .bind(py)
            .call1((&task.event_loop, task_object))
        {
            enter_error.print(py);
            task.finish(HandlerResponse::internal_error(), false);
            return;
        }
        // SAFETY: `context` is a valid contextvars.Context this task owns;
        // the matching PyContext_Exit below runs before this function
        // returns, and the context is only ever entered by this loop thread.
        if unsafe { ffi::PyContext_Enter(task.context.as_ptr()) } != 0 {
            PyErr::fetch(py).print(py);
            let _ = task
                .leave_task
                .bind(py)
                .call1((&task.event_loop, task_object));
            task.finish(HandlerResponse::internal_error(), false);
            return;
        }

        task.step_until_suspended(py, task_object, pending_throw);

        // SAFETY: balances the successful PyContext_Enter above.
        if unsafe { ffi::PyContext_Exit(task.context.as_ptr()) } != 0 {
            PyErr::fetch(py).print(py);
        }
        if let Err(leave_error) = task
            .leave_task
            .bind(py)
            .call1((&task.event_loop, task_object))
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
        self.event_loop.clone_ref(py)
    }

    fn get_name(&self) -> &'static str {
        "rayo-handler"
    }
}

/// The Python event-loop thread and the handles needed to schedule onto it.
pub struct EventLoop {
    loop_object: Py<PyAny>,
    enter_task: Py<PyAny>,
    leave_task: Py<PyAny>,
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
        let missing_task_hooks = |attribute_error: PyErr| {
            PyRuntimeError::new_err(format!(
                "Rayo needs asyncio.tasks._enter_task/_leave_task to register handler \
                 tasks with asyncio (present in every CPython since 3.7); this Python \
                 does not provide them: {attribute_error}"
            ))
        };
        let enter_task = tasks_module
            .getattr("_enter_task")
            .map_err(missing_task_hooks)?;
        let leave_task = tasks_module
            .getattr("_leave_task")
            .map_err(missing_task_hooks)?;

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
            enter_task: enter_task.unbind(),
            leave_task: leave_task.unbind(),
            thread: Mutex::new(Some(thread)),
            in_flight: Arc::new(AtomicUsize::new(0)),
        })
    }

    pub fn in_flight_count(&self) -> usize {
        self.in_flight.load(Ordering::Relaxed)
    }

    /// Build the handler coroutine and schedule its first step onto the
    /// event loop. Always resolves the returned request — dispatch failures
    /// become 500s, never hangs.
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

        // Calling an async function only builds the coroutine object — no
        // handler code runs. A failure here is a signature mismatch between
        // registration and dispatch: a Rayo bug, not a client error.
        let coroutine = match handler.bind(py).call((), Some(&handler_kwargs)) {
            Ok(coroutine) => coroutine,
            Err(call_error) => {
                call_error.print(py);
                channel.send(HandlerResponse::internal_error());
                return DispatchedRequest::failed(response_receiver);
            }
        };

        // SAFETY: PyContext_CopyCurrent returns a new strong reference to a
        // fresh Context (or null with an exception set); the thread is
        // attached (`py` witnesses it).
        let context_pointer = unsafe { ffi::PyContext_CopyCurrent() };
        if context_pointer.is_null() {
            PyErr::fetch(py).print(py);
            channel.send(HandlerResponse::internal_error());
            return DispatchedRequest::failed(response_receiver);
        }
        // SAFETY: non-null return from PyContext_CopyCurrent is a valid new
        // strong reference.
        let context = unsafe { Bound::from_owned_ptr(py, context_pointer) };

        let task = match Py::new(
            py,
            HandlerTask {
                coroutine: coroutine.unbind(),
                context: context.unbind(),
                event_loop: self.loop_object.clone_ref(py),
                enter_task: self.enter_task.clone_ref(py),
                leave_task: self.leave_task.clone_ref(py),
                channel,
                state: Mutex::new(HandlerTaskState::default()),
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

        let scheduled = self
            .loop_object
            .bind(py)
            .call_method1(intern!(py, "call_soon_threadsafe"), (&task,));
        if let Err(scheduling_error) = scheduled {
            scheduling_error.print(py);
            task.get().channel.send(HandlerResponse::internal_error());
            return DispatchedRequest::failed(response_receiver);
        }
        DispatchedRequest {
            response_receiver,
            task: Some(task),
            responded: false,
        }
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

/// A dispatched request as seen by the server task: await [`response`] for
/// the outcome. Dropping it before the response arrives cancels the handler —
/// this is how client disconnects propagate (Hyper drops the response future
/// when the connection dies).
///
/// [`response`]: DispatchedRequest::response
pub struct DispatchedRequest {
    response_receiver: oneshot::Receiver<HandlerResponse>,
    task: Option<Py<HandlerTask>>,
    responded: bool,
}

impl DispatchedRequest {
    /// Dispatch never reached the loop; the outcome (or the dropped sender)
    /// is already in the channel and there is nothing left to cancel.
    fn failed(response_receiver: oneshot::Receiver<HandlerResponse>) -> Self {
        Self {
            response_receiver,
            task: None,
            responded: true,
        }
    }

    /// Wait for the handler's response. `None` means the scheduler went away
    /// without responding (the event loop shut down mid-request).
    pub async fn response(&mut self) -> Option<HandlerResponse> {
        let outcome = (&mut self.response_receiver).await;
        self.responded = true;
        self.task = None;
        outcome.ok()
    }
}

impl Drop for DispatchedRequest {
    fn drop(&mut self) {
        if self.responded {
            return;
        }
        let Some(task) = self.task.take() else {
            return;
        };
        if task.get().lock_state().finished {
            return;
        }
        // The response future was dropped before completion — the client
        // disconnected. Propagate as cancellation onto the loop thread. A
        // failure here means the loop is closing; nothing left to cancel into.
        Python::attach(|py| {
            let _scheduled_or_shutting_down = task
                .bind(py)
                .getattr(intern!(py, "cancel"))
                .and_then(|cancel_method| {
                    task.get()
                        .event_loop
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
