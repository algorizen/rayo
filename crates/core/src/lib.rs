//! `rayo-core` — the PyO3 extension module that binds Rayo's Rust crates to
//! Python as `rayo._core`.
//!
//! This crate is the only place Python and Rust meet. The boundary rules are
//! hard invariants (ARCHITECTURE.md §6): one `Python::attach` per request,
//! `detach` around Rust work longer than 1 ms, and zero Python object
//! creation on the response serialization path.

use std::sync::{Arc, Mutex};
use std::time::Duration;

use bytes::Bytes;
use http_body_util::Full;
use hyper::body::Incoming;
use hyper::header::CONTENT_TYPE;
use hyper::{Request, Response, StatusCode};
use pyo3::exceptions::{PyRuntimeError, PyValueError};
use pyo3::prelude::*;
use pyo3::types::PyDict;
use rayo_dispatch::{EventLoopPool, HandlerResponse};
use rayo_server::{BoxedResponseFuture, RequestService, ResponseBody};
use tokio::sync::{oneshot, watch};

/// The version compiled into the Rust core. The Python layer checks this
/// against the installed package version at import time and fails loudly on
/// mismatch (invariant 5: nothing defers a broken install to request time).
#[pyfunction]
fn core_version() -> &'static str {
    env!("CARGO_PKG_VERSION")
}

/// How a path parameter converts before reaching the handler. Codes are
/// assigned by the Python surface (`rayo/_app.py`) at registration time.
#[derive(Clone, Copy)]
enum ParamKind {
    Str,
    Int,
    Float,
    Bool,
}

impl ParamKind {
    fn from_code(code: u8) -> Option<Self> {
        match code {
            0 => Some(Self::Str),
            1 => Some(Self::Int),
            2 => Some(Self::Float),
            3 => Some(Self::Bool),
            _ => None,
        }
    }

    fn expected_name(self) -> &'static str {
        match self {
            Self::Str => "str",
            Self::Int => "int",
            Self::Float => "float",
            Self::Bool => "bool",
        }
    }
}

struct RouteEntry {
    method: String,
    path: String,
    handler: Py<PyAny>,
    is_async: bool,
    params: Vec<(String, ParamKind)>,
}

/// Everything the request path needs, immutable once serving starts.
struct AppService {
    router: rayo_router::Router,
    routes: Vec<RouteEntry>,
    event_loops: EventLoopPool,
}

impl AppService {
    /// Build the handler kwargs from matched path segments — the input half
    /// of the boundary crossing. A conversion failure is the client's error:
    /// it becomes a 422 naming the parameter, the expected type, and the value.
    fn build_kwargs<'py>(
        &self,
        py: Python<'py>,
        route: &RouteEntry,
        matched_params: &[(String, String)],
    ) -> Result<Bound<'py, PyDict>, HandlerResponse> {
        let kwargs = PyDict::new(py);
        for (param_name, param_kind) in &route.params {
            let Some((_, raw_value)) = matched_params
                .iter()
                .find(|(matched_name, _)| matched_name == param_name)
            else {
                // Registration guarantees spec params ⊆ path params; reaching
                // here is a Rayo bug, not a client error.
                eprintln!(
                    "rayo-core: route {} {} lost path parameter {:?} between match and dispatch",
                    route.method, route.path, param_name
                );
                return Err(HandlerResponse::internal_error());
            };
            convert_and_insert(py, &kwargs, param_name, *param_kind, raw_value)?;
        }
        Ok(kwargs)
    }
}

/// 422 with a body that names what's wrong and where — error messages are
/// product. Generated in Rust; no Python objects involved.
fn validation_error_response(
    param_name: &str,
    expected: ParamKind,
    raw_value: &str,
) -> HandlerResponse {
    let mut body = Vec::with_capacity(96);
    body.extend_from_slice(b"{\"detail\":");
    let message = format!(
        "path parameter '{}' expected {}, got '{}'",
        param_name,
        expected.expected_name(),
        raw_value
    );
    rayo_schema::write_json_string(&message, &mut body);
    body.push(b'}');
    HandlerResponse {
        status: 422,
        content_type: "application/json",
        body,
    }
}

fn plain_response(status: StatusCode, body: &'static str) -> Response<ResponseBody> {
    let mut response = Response::new(Full::new(Bytes::from_static(body.as_bytes())));
    *response.status_mut() = status;
    response.headers_mut().insert(
        CONTENT_TYPE,
        hyper::header::HeaderValue::from_static("text/plain; charset=utf-8"),
    );
    response
}

fn response_from_handler(handler_response: HandlerResponse) -> Response<ResponseBody> {
    let mut response = Response::new(Full::new(Bytes::from(handler_response.body)));
    *response.status_mut() =
        StatusCode::from_u16(handler_response.status).unwrap_or(StatusCode::INTERNAL_SERVER_ERROR);
    match hyper::header::HeaderValue::from_str(handler_response.content_type) {
        Ok(content_type_value) => {
            response
                .headers_mut()
                .insert(CONTENT_TYPE, content_type_value);
        }
        Err(_) => {
            // Content types here are compile-time constants; this cannot fire.
        }
    }
    response
}

impl RequestService for AppService {
    fn handle(&self, request: Request<Incoming>) -> BoxedResponseFuture {
        // Routing is pure Rust — no Python involvement for misses.
        let Some(route_match) = self
            .router
            .lookup(request.method().as_str(), request.uri().path())
        else {
            return Box::pin(async { plain_response(StatusCode::NOT_FOUND, "Not Found") });
        };
        let route = &self.routes[route_match.handler_id];

        if route.is_async {
            // The single attach for this request: build inputs, hand the
            // handler coroutine to the scheduler, leave. The response arrives
            // as bytes through the channel. If Hyper drops the future before
            // it resolves (client disconnect), dropping the dispatched
            // request cancels the handler.
            let dispatch_outcome =
                Python::attach(
                    |py| match self.build_kwargs(py, route, &route_match.path_params) {
                        Ok(kwargs) => Ok(self.event_loops.schedule(py, &route.handler, kwargs)),
                        Err(error_response) => Err(error_response),
                    },
                );
            Box::pin(async move {
                match dispatch_outcome {
                    Ok(mut dispatched_request) => match dispatched_request.response().await {
                        Some(handler_response) => response_from_handler(handler_response),
                        None => plain_response(
                            StatusCode::INTERNAL_SERVER_ERROR,
                            "Internal Server Error",
                        ),
                    },
                    Err(error_response) => response_from_handler(error_response),
                }
            })
        } else {
            // Sync handlers may block arbitrarily long: keep them off the
            // Tokio workers. Thread-per-request topology on FT builds and the
            // configurable pool arrive with ADR-0006 work.
            let handler = Python::attach(|py| route.handler.clone_ref(py));
            let params_for_thread = route_match.path_params.clone();
            let param_specs: Vec<(String, ParamKind)> = route.params.clone();
            let route_label = (route.method.clone(), route.path.clone());
            let (response_sender, response_receiver) = oneshot::channel::<HandlerResponse>();
            tokio::task::spawn_blocking(move || {
                let handler_response = Python::attach(|py| {
                    let kwargs = PyDict::new(py);
                    for (param_name, param_kind) in &param_specs {
                        let Some((_, raw_value)) = params_for_thread
                            .iter()
                            .find(|(matched_name, _)| matched_name == param_name)
                        else {
                            eprintln!(
                                "rayo-core: route {} {} lost path parameter {:?}",
                                route_label.0, route_label.1, param_name
                            );
                            return HandlerResponse::internal_error();
                        };
                        let converted =
                            convert_and_insert(py, &kwargs, param_name, *param_kind, raw_value);
                        if let Err(error_response) = converted {
                            return error_response;
                        }
                    }
                    rayo_dispatch::execute_sync_handler(handler.bind(py), &kwargs)
                });
                // The ignored error is the documented "finish and discard"
                // half of sync-handler disconnect semantics (see
                // `execute_sync_handler` in rayo-dispatch): a disconnected
                // client drops the receiver, and the completed response is
                // silently discarded — never logged, never a panic.
                let _ = response_sender.send(handler_response);
            });
            Box::pin(async move {
                match response_receiver.await {
                    Ok(handler_response) => response_from_handler(handler_response),
                    Err(_sender_dropped) => {
                        plain_response(StatusCode::INTERNAL_SERVER_ERROR, "Internal Server Error")
                    }
                }
            })
        }
    }
}

fn convert_and_insert(
    py: Python<'_>,
    kwargs: &Bound<'_, PyDict>,
    param_name: &str,
    param_kind: ParamKind,
    raw_value: &str,
) -> Result<(), HandlerResponse> {
    let inserted = match param_kind {
        ParamKind::Str => kwargs.set_item(param_name, raw_value),
        ParamKind::Int => match raw_value.parse::<i64>() {
            Ok(integer_value) => kwargs.set_item(param_name, integer_value),
            Err(_) => return Err(validation_error_response(param_name, param_kind, raw_value)),
        },
        ParamKind::Float => match raw_value.parse::<f64>() {
            Ok(float_value) => kwargs.set_item(param_name, float_value),
            Err(_) => return Err(validation_error_response(param_name, param_kind, raw_value)),
        },
        ParamKind::Bool => match raw_value {
            value if value.eq_ignore_ascii_case("true") || value == "1" => {
                kwargs.set_item(param_name, true)
            }
            value if value.eq_ignore_ascii_case("false") || value == "0" => {
                kwargs.set_item(param_name, false)
            }
            _ => return Err(validation_error_response(param_name, param_kind, raw_value)),
        },
    };
    inserted.map_err(|insertion_error| {
        insertion_error.print(py);
        HandlerResponse::internal_error()
    })
}

/// How long `shutdown()` waits for in-flight requests to finish naturally
/// before cancelling them, when no explicit grace period is given.
const DEFAULT_SHUTDOWN_GRACE_SECONDS: f64 = 30.0;
/// How long cancelled handlers get to unwind and resolve their responses.
const CANCELLED_DRAIN_SECONDS: f64 = 5.0;

/// A running Rayo server. Frozen and fully `Sync`: usable from any thread on
/// free-threaded builds.
#[pyclass(frozen)]
struct Server {
    port: u16,
    runtime: tokio::runtime::Runtime,
    shutdown_sender: watch::Sender<bool>,
    serve_finished: Mutex<Option<oneshot::Receiver<()>>>,
    service: Arc<AppService>,
}

impl Server {
    /// Stop accepting, drain in-flight requests within the grace period,
    /// cancel whatever is still running, stop the event loops. Idempotent;
    /// every blocking wait detaches so loop threads stay able to attach.
    fn shutdown_impl(&self, py: Python<'_>, grace_seconds: Option<f64>) {
        let _ = self.shutdown_sender.send(true);
        let finished_receiver = {
            let mut finished_slot = self
                .serve_finished
                .lock()
                .unwrap_or_else(|poisoned_lock| poisoned_lock.into_inner());
            finished_slot.take()
        };
        if let Some(mut receiver) = finished_receiver {
            let natural_grace = Duration::from_secs_f64(
                grace_seconds
                    .unwrap_or(DEFAULT_SHUTDOWN_GRACE_SECONDS)
                    .max(0.0),
            );
            let drained_naturally = py.detach(|| {
                self.runtime.block_on(async {
                    tokio::time::timeout(natural_grace, &mut receiver)
                        .await
                        .is_ok()
                })
            });
            if !drained_naturally {
                // Grace expired with handlers still running: cancel them so
                // their cleanup executes and the connection drain can end.
                self.service.event_loops.cancel_in_flight(py);
                let drained_after_cancel = py.detach(|| {
                    self.runtime.block_on(async {
                        tokio::time::timeout(
                            Duration::from_secs_f64(CANCELLED_DRAIN_SECONDS),
                            &mut receiver,
                        )
                        .await
                        .is_ok()
                    })
                });
                if !drained_after_cancel {
                    eprintln!("rayo: shutdown is proceeding with connections still draining");
                }
            }
        }
        self.service.event_loops.stop(py);
    }
}

#[pymethods]
impl Server {
    #[getter]
    fn port(&self) -> u16 {
        self.port
    }

    /// Async requests currently being handled (scheduled, not yet completed).
    /// A live ops gauge; approximate under concurrency by nature.
    #[getter]
    fn in_flight(&self) -> usize {
        self.service.event_loops.total_in_flight()
    }

    /// Block until Ctrl+C (or a `shutdown()` from another thread), then shut
    /// down gracefully.
    fn wait(&self, py: Python<'_>) {
        let mut shutdown_watch = self.shutdown_sender.subscribe();
        py.detach(|| {
            self.runtime.block_on(async {
                tokio::select! {
                    _ = tokio::signal::ctrl_c() => {}
                    _ = shutdown_watch.changed() => {}
                }
            });
        });
        self.shutdown_impl(py, None);
    }

    /// Graceful shutdown: in-flight requests get `grace_seconds` (default
    /// 30) to finish, then are cancelled so their cleanup runs. Idempotent.
    #[pyo3(signature = (grace_seconds = None))]
    fn shutdown(&self, py: Python<'_>, grace_seconds: Option<f64>) {
        self.shutdown_impl(py, grace_seconds);
    }
}

impl Drop for Server {
    fn drop(&mut self) {
        // Reached without an explicit shutdown() when the Server object is
        // collected. Zero grace disposes of stragglers immediately, and the
        // detached waits inside keep this safe with the GIL held — the tokio
        // workers it waits on may themselves need to attach.
        Python::attach(|py| self.shutdown_impl(py, Some(0.0)));
    }
}

type RouteSpec = (String, String, Py<PyAny>, bool, Vec<(String, u8)>);

/// Bind, start serving on a background runtime, and return the handle.
/// Every failure here is a startup failure: specific and immediate.
/// `loop_threads` is resolved by the Python surface (cores on free-threaded
/// builds, 1 on GIL builds) so the policy lives next to its documentation.
#[pyfunction]
fn start_server(
    py: Python<'_>,
    host: &str,
    port: u16,
    routes: Vec<RouteSpec>,
    loop_threads: usize,
) -> PyResult<Server> {
    let mut router = rayo_router::Router::new();
    let mut route_entries = Vec::with_capacity(routes.len());
    for (route_index, (method, path, handler, is_async, raw_params)) in
        routes.into_iter().enumerate()
    {
        router
            .add_route(&method, &path, route_index)
            .map_err(|registration_error| PyValueError::new_err(registration_error.to_string()))?;
        let mut params = Vec::with_capacity(raw_params.len());
        for (param_name, kind_code) in raw_params {
            let param_kind = ParamKind::from_code(kind_code).ok_or_else(|| {
                PyValueError::new_err(format!(
                    "route {method} {path}: unknown parameter kind code {kind_code} for \
                     '{param_name}' — the Python and Rust layers disagree; reinstall rayo"
                ))
            })?;
            params.push((param_name, param_kind));
        }
        route_entries.push(RouteEntry {
            method,
            path,
            handler,
            is_async,
            params,
        });
    }

    let event_loops = EventLoopPool::start(py, loop_threads)?;
    let service = Arc::new(AppService {
        router,
        routes: route_entries,
        event_loops,
    });

    let runtime = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .thread_name("rayo-server")
        .build()
        .map_err(|runtime_error| {
            PyRuntimeError::new_err(format!("Rayo could not start its runtime: {runtime_error}"))
        })?;

    let listener = runtime
        .block_on(tokio::net::TcpListener::bind((host, port)))
        .map_err(|bind_error| {
            PyRuntimeError::new_err(format!(
                "Rayo could not bind http://{host}:{port}: {bind_error}"
            ))
        })?;
    let bound_port = listener
        .local_addr()
        .map_err(|address_error| {
            PyRuntimeError::new_err(format!(
                "Rayo could not read the bound address for http://{host}:{port}: {address_error}"
            ))
        })?
        .port();

    let (shutdown_sender, shutdown_receiver) = watch::channel(false);
    let (finished_sender, finished_receiver) = oneshot::channel();
    let service_for_serving = Arc::clone(&service);
    runtime.spawn(async move {
        rayo_server::serve(listener, service_for_serving, shutdown_receiver).await;
        let _ = finished_sender.send(());
    });

    Ok(Server {
        port: bound_port,
        runtime,
        shutdown_sender,
        serve_finished: Mutex::new(Some(finished_receiver)),
        service,
    })
}

/// `rayo._core` — declared `gil_used = false`: nothing in this module may
/// rely on the GIL for correctness (ADR-0002, invariant 4).
#[pymodule(gil_used = false)]
fn _core(module: &Bound<'_, PyModule>) -> PyResult<()> {
    module.add_function(wrap_pyfunction!(core_version, module)?)?;
    module.add_function(wrap_pyfunction!(start_server, module)?)?;
    module.add_class::<Server>()?;
    module.add_class::<rayo_dispatch::HandlerTask>()?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn core_version_matches_cargo_manifest() {
        assert_eq!(core_version(), env!("CARGO_PKG_VERSION"));
    }

    #[test]
    fn validation_error_body_is_json_with_detail() {
        let response = validation_error_response("user_id", ParamKind::Int, "abc");
        assert_eq!(response.status, 422);
        let body_text = String::from_utf8(response.body)
            .unwrap_or_else(|invalid| panic!("non-UTF-8 body: {invalid}"));
        assert_eq!(
            body_text,
            "{\"detail\":\"path parameter 'user_id' expected int, got 'abc'\"}"
        );
    }
}
