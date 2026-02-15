use crate::broker::config::TelemetryConfig;
use opentelemetry::trace::TracerProvider as _;
use opentelemetry::KeyValue;
use opentelemetry_otlp::WithExportConfig;
use opentelemetry_sdk::metrics::{PeriodicReader, SdkMeterProvider};
use opentelemetry_sdk::trace::SdkTracerProvider;
use opentelemetry_sdk::Resource;
use tracing_subscriber::layer::SubscriberExt;
use tracing_subscriber::util::SubscriberInitExt;
use tracing_subscriber::EnvFilter;

/// Guard that flushes and shuts down OTel providers on drop.
/// Hold this in `main` and call `shutdown()` during graceful shutdown.
pub struct TelemetryGuard {
    tracer_provider: SdkTracerProvider,
    meter_provider: SdkMeterProvider,
}

impl TelemetryGuard {
    /// Flush pending spans/metrics and shut down the OTel pipeline.
    pub fn shutdown(self) {
        if let Err(e) = self.tracer_provider.shutdown() {
            eprintln!("OTel tracer shutdown error: {e}");
        }
        if let Err(e) = self.meter_provider.shutdown() {
            eprintln!("OTel meter shutdown error: {e}");
        }
    }
}

/// Initialize telemetry: structured logging + optional OTel export.
///
/// When `config.otlp_endpoint` is set, an OTLP gRPC exporter is configured
/// for both traces and metrics. When absent, only the tracing-subscriber
/// fmt layer is installed (plain logging, no export).
///
/// Returns `Some(TelemetryGuard)` when OTel is active, `None` otherwise.
pub fn init_telemetry(config: &TelemetryConfig) -> Option<TelemetryGuard> {
    let filter = EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("info"));

    if let Some(endpoint) = &config.otlp_endpoint {
        match init_otel_pipeline(config, endpoint) {
            Ok((guard, otel_layer)) => {
                let fmt_layer = fmt_layer();
                // OTel layer must be added to Registry first (it implements Layer<Registry>),
                // then filter and fmt are layered on top.
                tracing_subscriber::registry()
                    .with(otel_layer)
                    .with(fmt_layer)
                    .with(filter)
                    .init();
                tracing::info!(
                    otlp_endpoint = %endpoint,
                    service_name = %config.service_name(),
                    metrics_interval_ms = config.metrics_interval_ms.unwrap_or(10_000),
                    "OTel telemetry initialized"
                );
                Some(guard)
            }
            Err(e) => {
                // Telemetry failure must not crash the broker
                let fmt_layer = fmt_layer();
                tracing_subscriber::registry()
                    .with(fmt_layer)
                    .with(filter)
                    .init();
                tracing::warn!(error = %e, "failed to initialize OTel pipeline, continuing without telemetry export");
                None
            }
        }
    } else {
        let fmt_layer = fmt_layer();
        tracing_subscriber::registry()
            .with(fmt_layer)
            .with(filter)
            .init();
        None
    }
}

fn fmt_layer<S>() -> tracing_subscriber::fmt::Layer<S>
where
    S: tracing::Subscriber + for<'a> tracing_subscriber::registry::LookupSpan<'a>,
{
    tracing_subscriber::fmt::layer().with_target(true)
}

fn init_otel_pipeline(
    config: &TelemetryConfig,
    endpoint: &str,
) -> Result<
    (
        TelemetryGuard,
        tracing_opentelemetry::OpenTelemetryLayer<
            tracing_subscriber::Registry,
            opentelemetry_sdk::trace::SdkTracer,
        >,
    ),
    Box<dyn std::error::Error + Send + Sync>,
> {
    let resource = Resource::builder()
        .with_attributes([KeyValue::new(
            "service.name",
            config.service_name().to_string(),
        )])
        .build();

    // Trace exporter
    let trace_exporter = opentelemetry_otlp::SpanExporter::builder()
        .with_tonic()
        .with_endpoint(endpoint)
        .build()?;

    let tracer_provider = SdkTracerProvider::builder()
        .with_batch_exporter(trace_exporter)
        .with_resource(resource.clone())
        .build();

    let tracer = tracer_provider.tracer("fila");
    let otel_layer = tracing_opentelemetry::layer().with_tracer(tracer);

    // Metrics exporter
    let metrics_exporter = opentelemetry_otlp::MetricExporter::builder()
        .with_tonic()
        .with_endpoint(endpoint)
        .build()?;

    let metrics_reader = PeriodicReader::builder(metrics_exporter)
        .with_interval(config.metrics_interval())
        .build();

    let meter_provider = SdkMeterProvider::builder()
        .with_reader(metrics_reader)
        .with_resource(resource)
        .build();

    opentelemetry::global::set_meter_provider(meter_provider.clone());

    // Set W3C TraceContext propagator for gRPC metadata extraction/injection
    opentelemetry::global::set_text_map_propagator(
        opentelemetry_sdk::propagation::TraceContextPropagator::new(),
    );

    let guard = TelemetryGuard {
        tracer_provider,
        meter_provider,
    };

    Ok((guard, otel_layer))
}
