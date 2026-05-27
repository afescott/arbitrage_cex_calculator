//! `tracing_subscriber` setup, with optional OTLP/gRPC export to a local Jaeger.
//!
//! Two layouts:
//! - **default build**: plain `fmt` layer at INFO; behavior identical to the old
//!   `tracing_subscriber::fmt().init()`.
//! - **`--features otel`**: `fmt` + `tracing_opentelemetry` layer, exporting via OTLP
//!   gRPC in a `BatchSpanProcessor`. The returned [`OtelGuard`] flushes/shuts down the
//!   provider on drop — keep it alive until after the last `await` in `main`.
//!
//! Env knobs (otel build only):
//! - `OTEL_EXPORTER_OTLP_ENDPOINT` (default `http://localhost:4317`)
//! - `OTEL_SERVICE_NAME` (default `security_flamegraph_lowlatency`)
//! - `RUST_LOG` (filter for both fmt + otel layers; default `info`)

use tracing_subscriber::layer::SubscriberExt;
use tracing_subscriber::util::SubscriberInitExt;
use tracing_subscriber::EnvFilter;

#[cfg(feature = "otel")]
pub struct OtelGuard {
    provider: opentelemetry_sdk::trace::SdkTracerProvider,
}

#[cfg(feature = "otel")]
impl Drop for OtelGuard {
    fn drop(&mut self) {
        // Best-effort flush. If Jaeger isn't reachable we don't want to panic on shutdown.
        if let Err(e) = self.provider.shutdown() {
            eprintln!("otel: tracer provider shutdown error: {e:?}");
        }
    }
}

#[cfg(not(feature = "otel"))]
pub struct OtelGuard;

/// Initialize the global tracing subscriber. Call exactly once at startup.
///
/// Returns a guard that must be kept alive for the lifetime of the program (or at
/// least until the last span you want exported); dropping it flushes pending spans.
pub fn init_subscriber() -> OtelGuard {
    let env_filter =
        EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("info"));
    let fmt_layer = tracing_subscriber::fmt::layer().with_target(false);

    #[cfg(feature = "otel")]
    {
        use opentelemetry_otlp::WithExportConfig;

        let endpoint = std::env::var("OTEL_EXPORTER_OTLP_ENDPOINT")
            .unwrap_or_else(|_| "http://localhost:4317".to_string());
        let service_name = std::env::var("OTEL_SERVICE_NAME")
            .unwrap_or_else(|_| env!("CARGO_PKG_NAME").to_string());

        let exporter = opentelemetry_otlp::SpanExporter::builder()
            .with_tonic()
            .with_endpoint(&endpoint)
            .build()
            .expect("OTLP span exporter init");

        let resource = opentelemetry_sdk::Resource::builder()
            .with_service_name(service_name.clone())
            .build();

        let provider = opentelemetry_sdk::trace::SdkTracerProvider::builder()
            .with_batch_exporter(exporter)
            .with_resource(resource)
            .build();

        opentelemetry::global::set_tracer_provider(provider.clone());

        use opentelemetry::trace::TracerProvider as _;
        let tracer = provider.tracer(env!("CARGO_PKG_NAME"));
        let otel_layer = tracing_opentelemetry::layer().with_tracer(tracer);

        tracing_subscriber::registry()
            .with(env_filter)
            .with(fmt_layer)
            .with(otel_layer)
            .init();

        eprintln!(
            "otel: exporting traces to {endpoint} (service.name={service_name})"
        );

        OtelGuard { provider }
    }

    #[cfg(not(feature = "otel"))]
    {
        tracing_subscriber::registry()
            .with(env_filter)
            .with(fmt_layer)
            .init();
        OtelGuard
    }
}
