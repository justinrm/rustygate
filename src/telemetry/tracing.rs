use std::env;

use opentelemetry::{global, propagation::Injector, trace::TracerProvider};
use opentelemetry_otlp::WithExportConfig;
use opentelemetry_sdk::{trace::SdkTracerProvider, Resource};
use reqwest::header::{HeaderMap, HeaderName, HeaderValue};
use tracing_opentelemetry::OpenTelemetrySpanExt;
use tracing_subscriber::{layer::SubscriberExt, util::SubscriberInitExt, EnvFilter};

use crate::config::TelemetryConfig;

pub fn init_tracing(config: &TelemetryConfig) -> anyhow::Result<()> {
    let filter = EnvFilter::try_from_default_env()
        .unwrap_or_else(|_| EnvFilter::new("rustygate=info,tower_http=info"));
    let fmt_layer = tracing_subscriber::fmt::layer();

    let otlp_endpoint = config
        .otlp_endpoint
        .clone()
        .or_else(|| env::var("RUSTYGATE_OTLP_ENDPOINT").ok());

    if let Some(endpoint) = otlp_endpoint.as_deref() {
        let exporter = opentelemetry_otlp::SpanExporter::builder()
            .with_tonic()
            .with_endpoint(endpoint)
            .build()?;
        let provider = SdkTracerProvider::builder()
            .with_simple_exporter(exporter)
            .with_resource(
                Resource::builder()
                    .with_service_name(config.service_name.clone())
                    .build(),
            )
            .build();
        let tracer = provider.tracer(config.service_name.clone());
        global::set_tracer_provider(provider);

        tracing_subscriber::registry()
            .with(filter)
            .with(fmt_layer)
            .with(tracing_opentelemetry::layer().with_tracer(tracer))
            .try_init()?;
    } else {
        tracing_subscriber::registry()
            .with(filter)
            .with(fmt_layer)
            .try_init()?;
    }

    Ok(())
}

pub fn inject_trace_context(headers: &mut HeaderMap) {
    let context = tracing::Span::current().context();
    global::get_text_map_propagator(|propagator| {
        propagator.inject_context(&context, &mut HeaderInjector(headers));
    });
}

struct HeaderInjector<'a>(&'a mut HeaderMap);

impl Injector for HeaderInjector<'_> {
    fn set(&mut self, key: &str, value: String) {
        let Ok(name) = HeaderName::from_bytes(key.as_bytes()) else {
            return;
        };
        let Ok(value) = HeaderValue::from_str(&value) else {
            return;
        };
        self.0.insert(name, value);
    }
}

#[cfg(test)]
mod tests {
    use super::init_tracing;
    use crate::config::TelemetryConfig;

    #[test]
    fn tracing_setup_accepts_default_config() {
        let _ = init_tracing(&TelemetryConfig::default());
    }
}
