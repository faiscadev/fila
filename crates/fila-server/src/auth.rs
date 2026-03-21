//! Tower layer that enforces API key authentication on incoming gRPC requests.
//!
//! When `broker.auth_enabled` is `true`, every incoming RPC must include an
//! `authorization: Bearer <key>` metadata header. Requests without a valid key
//! receive `UNAUTHENTICATED`. When auth is disabled (the default), all requests
//! pass through unconditionally.
//!
//! No RPC bypasses authentication. To provision the first API key, configure
//! a `bootstrap_apikey` in `fila.toml` or set `FILA_BOOTSTRAP_APIKEY`. The
//! bootstrap key is accepted as a valid credential by `validate_api_key` without
//! a storage lookup, so operators can use it to call `CreateApiKey` and then
//! rotate it out once real keys are in place.
//!
//! When a request is authenticated, the middleware injects a `ValidatedKeyId`
//! extension into the HTTP request. Service handlers extract it to perform
//! per-queue ACL checks.

use std::sync::Arc;
use std::task::{Context as TaskContext, Poll};

use fila_core::Broker;
use tower::{Layer, Service};

/// Request extension injected by the auth middleware for authenticated requests.
///
/// Service handlers extract this to perform ACL checks via `broker.check_permission`.
/// Absent when auth is disabled.
#[derive(Clone, Debug)]
pub struct ValidatedKeyId(pub fila_core::broker::auth::CallerKey);

/// gRPC paths that bypass authentication.
/// `GetServerInfo` is unauthenticated so clients can query server version and
/// features before providing credentials (compatibility negotiation).
const AUTH_BYPASS_PATHS: &[&str] = &["/fila.v1.FilaAdmin/GetServerInfo"];

/// Tower layer that wraps services with API key authentication.
#[derive(Clone)]
pub struct AuthLayer {
    broker: Arc<Broker>,
}

impl AuthLayer {
    pub fn new(broker: Arc<Broker>) -> Self {
        Self { broker }
    }
}

impl<S> Layer<S> for AuthLayer {
    type Service = AuthService<S>;

    fn layer(&self, inner: S) -> Self::Service {
        AuthService {
            inner,
            broker: Arc::clone(&self.broker),
        }
    }
}

/// Tower service wrapper that validates the `authorization` header.
#[derive(Clone)]
pub struct AuthService<S> {
    inner: S,
    broker: Arc<Broker>,
}

impl<S, B> Service<http::Request<B>> for AuthService<S>
where
    S: Service<http::Request<B>, Response = http::Response<tonic::body::Body>>
        + Clone
        + Send
        + 'static,
    S::Future: Send + 'static,
    B: Send + 'static,
{
    type Response = S::Response;
    type Error = S::Error;
    type Future = futures_core::future::BoxFuture<'static, Result<Self::Response, Self::Error>>;

    fn poll_ready(&mut self, cx: &mut TaskContext<'_>) -> Poll<Result<(), Self::Error>> {
        self.inner.poll_ready(cx)
    }

    fn call(&mut self, mut req: http::Request<B>) -> Self::Future {
        // Take the readied service and replace it with a clone so that
        // `self.inner` is ready for the next poll_ready/call cycle.
        // This satisfies the Tower contract: the instance that passed
        // poll_ready must be the one used for the actual call.
        let clone = self.inner.clone();
        let mut inner = std::mem::replace(&mut self.inner, clone);

        // When auth is disabled, pass through unconditionally.
        if !self.broker.auth_enabled {
            return Box::pin(inner.call(req));
        }

        // Check bypass paths (currently empty — all RPCs require auth).
        let path = req.uri().path();
        for bypass in AUTH_BYPASS_PATHS {
            if path == *bypass {
                return Box::pin(inner.call(req));
            }
        }

        // Extract the Bearer token from the `authorization` metadata header.
        let token = req
            .headers()
            .get("authorization")
            .and_then(|v| v.to_str().ok())
            .and_then(|s| s.strip_prefix("Bearer "))
            .map(|s| s.to_string());

        let broker = Arc::clone(&self.broker);

        Box::pin(async move {
            let token = match token {
                Some(t) => t,
                None => {
                    return Ok(unauthenticated_response());
                }
            };

            match broker.validate_api_key(&token) {
                Ok(Some(caller)) => {
                    req.extensions_mut().insert(ValidatedKeyId(caller));
                    inner.call(req).await
                }
                Ok(None) => Ok(unauthenticated_response()),
                Err(e) => {
                    tracing::error!(error = %e, "auth storage error");
                    Ok(internal_error_response())
                }
            }
        })
    }
}

fn unauthenticated_response() -> http::Response<tonic::body::Body> {
    tonic::Status::unauthenticated("invalid or missing api key").into_http()
}

fn internal_error_response() -> http::Response<tonic::body::Body> {
    tonic::Status::internal("authentication service unavailable").into_http()
}
