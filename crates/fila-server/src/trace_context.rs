//! Tower layer that extracts W3C trace context (traceparent/tracestate)
//! from incoming gRPC request HTTP headers and propagates it to the
//! current OpenTelemetry context.
//!
//! This completes the deferred item from Story 6.1: the W3C
//! TraceContextPropagator is registered globally in `telemetry.rs`,
//! and this layer uses it to extract parent context from incoming
//! requests so that handler spans link to the caller's trace.

use std::task::{Context as TaskContext, Poll};

use opentelemetry::propagation::Extractor;
use tower::{Layer, Service};

/// Extracts W3C trace context headers from an HTTP header map.
struct HeaderExtractor<'a>(&'a http::HeaderMap);

impl Extractor for HeaderExtractor<'_> {
    fn get(&self, key: &str) -> Option<&str> {
        self.0.get(key).and_then(|v| v.to_str().ok())
    }

    fn keys(&self) -> Vec<&str> {
        self.0.keys().map(|k| k.as_str()).collect()
    }
}

/// Tower layer that wraps services with W3C trace context extraction.
#[derive(Clone)]
pub struct TraceContextLayer;

impl<S> Layer<S> for TraceContextLayer {
    type Service = TraceContextService<S>;

    fn layer(&self, inner: S) -> Self::Service {
        TraceContextService { inner }
    }
}

/// Tower service wrapper that extracts W3C trace context from incoming
/// HTTP headers and creates a parent tracing span linked to the caller's
/// trace. All downstream handler spans become children of this span.
#[derive(Clone)]
pub struct TraceContextService<S> {
    inner: S,
}

impl<S, B> Service<http::Request<B>> for TraceContextService<S>
where
    S: Service<http::Request<B>>,
{
    type Response = S::Response;
    type Error = S::Error;
    type Future = Instrumented<S::Future>;

    fn poll_ready(&mut self, cx: &mut TaskContext<'_>) -> Poll<Result<(), Self::Error>> {
        self.inner.poll_ready(cx)
    }

    fn call(&mut self, req: http::Request<B>) -> Self::Future {
        // Extract W3C trace context from HTTP headers using the globally
        // registered TraceContextPropagator (set in telemetry.rs).
        let parent_cx = opentelemetry::global::get_text_map_propagator(|prop| {
            prop.extract(&HeaderExtractor(req.headers()))
        });

        // Attach the extracted context as current so the tracing span
        // picks it up as its parent (via tracing-opentelemetry).
        let _guard = parent_cx.attach();
        let span = tracing::info_span!("grpc.server");

        // The span now holds a reference to the parent context; the guard
        // can be dropped safely. Instrument the inner future with this span
        // so all handler spans become children.
        Instrumented {
            inner: self.inner.call(req),
            span,
        }
    }
}

// Future wrapper that instruments an inner future with a tracing span.
pin_project_lite::pin_project! {
    pub struct Instrumented<F> {
        #[pin]
        inner: F,
        span: tracing::Span,
    }
}

impl<F: std::future::Future> std::future::Future for Instrumented<F> {
    type Output = F::Output;

    fn poll(self: std::pin::Pin<&mut Self>, cx: &mut TaskContext<'_>) -> Poll<Self::Output> {
        let this = self.project();
        let _enter = this.span.enter();
        this.inner.poll(cx)
    }
}
