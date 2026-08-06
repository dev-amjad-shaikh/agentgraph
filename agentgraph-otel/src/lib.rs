//! # agentgraph-otel
//!
//! OpenTelemetry export layer for the [`agentgraph`] engine.
//!
//! `agentgraph` already emits structured `tracing` spans from its executor
//! (`agentgraph.run` → `agentgraph.super_step` → `agentgraph.node`). This
//! crate wires a global [`tracing_subscriber`] that routes those spans to
//! two sinks:
//!
//! - a human-readable `fmt` layer on stderr (always on), gated by an
//!   [`EnvFilter`], and
//! - an OTLP/HTTP span exporter (only when [`OTelConfig::otlp_endpoint`] is
//!   set), so the same spans show up in a collector / Jaeger / any
//!   OTLP-compatible backend.
//!
//! ## Quick start
//!
//! ```no_run
//! // Local development: pretty logs only.
//! let _guard = agentgraph_otel::init_local("my-agent").unwrap();
//!
//! // With a collector: logs + OTLP spans.
//! let _guard = agentgraph_otel::init(agentgraph_otel::OTelConfig {
//!     service_name: "my-agent".into(),
//!     otlp_endpoint: Some("http://localhost:4318/v1/traces".into()),
//!     log_filter: None, // RUST_LOG, defaulting to a sensible agentgraph filter
//! })
//! .unwrap();
//! ```
//!
//! Keep the returned [`OTelGuard`] alive for the duration of the process and
//! call [`OTelGuard::shutdown`] before exit to flush any buffered spans.

use opentelemetry::global;
use opentelemetry::trace::TracerProvider;
use opentelemetry_otlp::{SpanExporter, WithExportConfig};
use opentelemetry_sdk::trace::SdkTracerProvider;
use opentelemetry_sdk::Resource;
use tracing_subscriber::layer::SubscriberExt;
use tracing_subscriber::util::SubscriberInitExt;
use tracing_subscriber::{fmt, EnvFilter, Registry};

/// Default log filter used when neither [`OTelConfig::log_filter`] nor the
/// `RUST_LOG` environment variable provides one. `agentgraph.super_step`
/// spans are emitted at DEBUG, so the `agentgraph=debug` directive surfaces
/// the full span taxonomy locally without drowning other crates in noise.
pub const DEFAULT_FILTER: &str = "info,agentgraph=debug";

/// Errors returned by [`init`] and [`init_local`].
#[derive(Debug, thiserror::Error)]
pub enum OTelError {
    /// A global tracing subscriber is already installed. `init` may only be
    /// called once per process; the second call fails with this error.
    #[error("a global tracing subscriber is already installed")]
    SubscriberAlreadyInstalled,
    /// The OTLP span exporter could not be built (bad endpoint, transport
    /// configuration failure, ...).
    #[error("failed to build the OTLP span exporter: {0}")]
    ExporterBuild(String),
}

/// Convenient result alias for this crate.
pub type Result<T> = std::result::Result<T, OTelError>;

/// Configuration for [`init`].
#[derive(Debug, Clone, Default)]
pub struct OTelConfig {
    /// Logical service name attached to every exported span as the OTel
    /// `service.name` resource attribute (e.g. `"support-agent"`).
    pub service_name: String,
    /// OTLP/HTTP endpoint for span export, e.g.
    /// `http://localhost:4318/v1/traces`. When `None`, no OTLP exporter is
    /// installed and only the local `fmt` layer runs.
    pub otlp_endpoint: Option<String>,
    /// Log/span filter directive (the `EnvFilter` syntax, e.g.
    /// `"info,agentgraph=trace"`). When `None`, the `RUST_LOG` environment
    /// variable is honored, falling back to [`DEFAULT_FILTER`].
    pub log_filter: Option<String>,
}

impl OTelConfig {
    /// Resolve the effective [`EnvFilter`]: explicit `log_filter` first,
    /// then `RUST_LOG`, then [`DEFAULT_FILTER`].
    fn env_filter(&self) -> EnvFilter {
        match &self.log_filter {
            Some(directive) => EnvFilter::new(directive),
            None => {
                EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new(DEFAULT_FILTER))
            }
        }
    }
}

/// Handle returned by [`init`]/[`init_local`]. Shuts down the tracer
/// provider (flushing any buffered spans) on [`OTelGuard::shutdown`] or on
/// drop. Shutdown is idempotent.
#[derive(Debug)]
pub struct OTelGuard {
    provider: Option<SdkTracerProvider>,
}

impl OTelGuard {
    /// Flush and shut down the tracer provider. Calling this more than once
    /// is a no-op after the first call; dropping the guard after an explicit
    /// shutdown is also safe.
    pub fn shutdown(&mut self) {
        if let Some(provider) = self.provider.take() {
            // A shutdown error only means spans may be lost on the way out;
            // there is nothing useful to recover into, so log and move on.
            if let Err(err) = provider.shutdown() {
                eprintln!("agentgraph-otel: tracer provider shutdown error: {err}");
            }
        }
    }
}

impl Drop for OTelGuard {
    fn drop(&mut self) {
        self.shutdown();
    }
}

/// Install the global tracing subscriber.
///
/// The subscriber is `Registry + EnvFilter + fmt layer (+ OTLP span layer
/// when `config.otlp_endpoint` is `Some`)`. Because the subscriber is global,
/// this function may only succeed **once per process**; a second call
/// returns [`OTelError::SubscriberAlreadyInstalled`] and leaves the existing
/// subscriber untouched.
///
/// The OTLP exporter uses a batch span processor running on its own
/// dedicated thread, so `init` works both inside and outside a Tokio
/// runtime.
pub fn init(config: OTelConfig) -> Result<OTelGuard> {
    let filter = config.env_filter();
    let fmt_layer = fmt::layer().with_writer(std::io::stderr);

    match &config.otlp_endpoint {
        Some(endpoint) => {
            let provider = build_tracer_provider(&config.service_name, endpoint)?;
            global::set_tracer_provider(provider.clone());
            let tracer = provider.tracer("agentgraph-otel");
            let otel_layer = tracing_opentelemetry::layer().with_tracer(tracer);
            Registry::default()
                .with(filter)
                .with(fmt_layer)
                .with(otel_layer)
                .try_init()
                .map_err(|_| OTelError::SubscriberAlreadyInstalled)?;
            Ok(OTelGuard {
                provider: Some(provider),
            })
        }
        None => {
            Registry::default()
                .with(filter)
                .with(fmt_layer)
                .try_init()
                .map_err(|_| OTelError::SubscriberAlreadyInstalled)?;
            Ok(OTelGuard { provider: None })
        }
    }
}

/// Install the global tracing subscriber with a local-only configuration:
/// `fmt` layer + [`EnvFilter`], no OTLP export. Equivalent to calling
/// [`init`] with only a service name.
pub fn init_local(service_name: &str) -> Result<OTelGuard> {
    init(OTelConfig {
        service_name: service_name.to_owned(),
        otlp_endpoint: None,
        log_filter: None,
    })
}

/// Build an [`SdkTracerProvider`] with a batch OTLP/HTTP span exporter and a
/// `service.name` resource attribute.
fn build_tracer_provider(service_name: &str, endpoint: &str) -> Result<SdkTracerProvider> {
    let exporter = SpanExporter::builder()
        .with_http()
        .with_endpoint(endpoint)
        .build()
        .map_err(|e| OTelError::ExporterBuild(e.to_string()))?;

    let resource = Resource::builder()
        .with_service_name(service_name.to_owned())
        .build();

    let provider = SdkTracerProvider::builder()
        .with_batch_exporter(exporter)
        .with_resource(resource)
        .build();
    Ok(provider)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn explicit_log_filter_wins() {
        let config = OTelConfig {
            service_name: "svc".into(),
            otlp_endpoint: None,
            log_filter: Some("agentgraph=trace".into()),
        };
        assert_eq!(config.env_filter().to_string(), "agentgraph=trace");
    }

    #[test]
    fn filter_defaults_when_unset_and_no_rust_log() {
        // Only meaningful when RUST_LOG is absent; when the harness exports
        // RUST_LOG the filter honors it instead of DEFAULT_FILTER. Either
        // way the result must be a valid, non-empty directive set.
        let config = OTelConfig::default();
        let filter = config.env_filter().to_string();
        assert!(!filter.is_empty());
        if std::env::var_os("RUST_LOG").is_none() {
            // EnvFilter normalizes directive ordering in its Display impl,
            // so compare the directive set rather than the raw string.
            let mut got: Vec<&str> = filter.split(',').collect();
            let mut want: Vec<&str> = DEFAULT_FILTER.split(',').collect();
            got.sort_unstable();
            want.sort_unstable();
            assert_eq!(got, want);
        }
    }

    #[test]
    fn default_filter_includes_agentgraph_debug() {
        assert!(DEFAULT_FILTER.contains("agentgraph=debug"));
    }
}
