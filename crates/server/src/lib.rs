//! `rayo-server` — the embedded HTTP server: Tokio + Hyper, HTTP/1.1 and
//! HTTP/2 (auto-negotiated), graceful shutdown. See ADR-0003.
//!
//! This crate is deliberately Python-free: it accepts any [`RequestService`]
//! and can be exercised in pure-Rust tests. The PyO3 wiring lives in
//! `rayo-core`. TLS and the worker topologies (ADR-0006) land later in M1.

#![forbid(unsafe_code)]

use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;

use bytes::Bytes;
use http_body_util::Full;
use hyper::body::Incoming;
use hyper::{Request, Response};
use hyper_util::rt::{TokioExecutor, TokioIo};
use tokio::net::TcpListener;
use tokio::sync::watch;

pub type ResponseBody = Full<Bytes>;
pub type BoxedResponseFuture = Pin<Box<dyn Future<Output = Response<ResponseBody>> + Send>>;

/// What the server needs from the layers above it: turn a request into a
/// response future. Implementations must be infallible at this boundary —
/// errors become error *responses*, never dropped connections.
pub trait RequestService: Send + Sync + 'static {
    fn handle(&self, request: Request<Incoming>) -> BoxedResponseFuture;
}

/// Accept connections until `shutdown_signal` fires, then drain in-flight
/// requests before returning (graceful shutdown).
pub async fn serve<S: RequestService>(
    listener: TcpListener,
    service: Arc<S>,
    mut shutdown_signal: watch::Receiver<bool>,
) {
    let graceful = hyper_util::server::graceful::GracefulShutdown::new();
    let connection_builder = hyper_util::server::conn::auto::Builder::new(TokioExecutor::new());

    loop {
        tokio::select! {
            accepted = listener.accept() => {
                let (stream, _peer_address) = match accepted {
                    Ok(connection_pair) => connection_pair,
                    Err(accept_error) => {
                        eprintln!("rayo-server: failed to accept a connection: {accept_error}");
                        continue;
                    }
                };
                let service_for_connection = Arc::clone(&service);
                let hyper_service = hyper::service::service_fn(move |request: Request<Incoming>| {
                    let service_for_request = Arc::clone(&service_for_connection);
                    async move {
                        Ok::<_, std::convert::Infallible>(service_for_request.handle(request).await)
                    }
                });
                let connection = connection_builder
                    .serve_connection_with_upgrades(TokioIo::new(stream), hyper_service);
                let watched_connection = graceful.watch(connection.into_owned());
                tokio::spawn(async move {
                    if let Err(connection_error) = watched_connection.await {
                        // Client disconnects and protocol errors are normal at
                        // this layer; surfaced for now, structured logging later.
                        eprintln!("rayo-server: connection ended with error: {connection_error}");
                    }
                });
            }
            _ = shutdown_signal.changed() => break,
        }
    }

    graceful.shutdown().await;
}
