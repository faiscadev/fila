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

/// gRPC paths that bypass authentication.
///
/// Only `CreateApiKey` is exempt — it must be reachable without a key so
/// operators can issue the first key (bootstrap problem). Once a key exists,
/// `RevokeApiKey` and `ListApiKeys` require a valid key.
const AUTH_BYPASS_PATHS: &[&str] = &["/fila.v1.FilaAdmin/CreateApiKey"];

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

        // `CreateApiKey` bypasses authentication (bootstrap).
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
