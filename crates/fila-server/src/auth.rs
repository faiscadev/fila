//! Tower layer that enforces API key authentication on incoming gRPC requests.
//!
//! When `broker.auth_enabled` is `true`, every incoming RPC must include an
//! `authorization: Bearer <key>` metadata header. Requests without a valid key
//! receive `UNAUTHENTICATED`. When auth is disabled (the default), all requests
//! pass through unconditionally.
//!
//! Key-management RPCs (`CreateApiKey`, `RevokeApiKey`, `ListApiKeys`) always
//! bypass authentication — they are the bootstrap mechanism for issuing keys.

use std::sync::Arc;
use std::task::{Context as TaskContext, Poll};

use fila_core::Broker;
use tower::{Layer, Service};

/// gRPC path suffixes that bypass authentication.
/// These are the key-management RPCs — they must be accessible without a key
/// so operators can issue the first key.
const AUTH_BYPASS_PATHS: &[&str] = &[
    "/fila.v1.FilaAdmin/CreateApiKey",
    "/fila.v1.FilaAdmin/RevokeApiKey",
    "/fila.v1.FilaAdmin/ListApiKeys",
];

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

    fn call(&mut self, req: http::Request<B>) -> Self::Future {
        // When auth is disabled, pass through unconditionally.
        if !self.broker.auth_enabled {
            let fut = self.inner.call(req);
            return Box::pin(fut);
        }

        // Key-management RPCs bypass authentication.
        let path = req.uri().path();
        for bypass in AUTH_BYPASS_PATHS {
            if path == *bypass {
                let fut = self.inner.call(req);
                return Box::pin(fut);
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
        let mut inner = self.inner.clone();

        Box::pin(async move {
            let token = match token {
                Some(t) => t,
                None => {
                    return Ok(unauthenticated_response());
                }
            };

            match broker.validate_api_key(&token) {
                Ok(true) => inner.call(req).await,
                Ok(false) => Ok(unauthenticated_response()),
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
