use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::OnceLock;
use std::time::Duration;

use anyhow::Result;
use opentelemetry::trace::TracerProvider as _;
use opentelemetry::{global, KeyValue};
use opentelemetry_otlp::{HasExportConfig, MetricExporter, SpanExporter};
use opentelemetry_sdk::{
    metrics::{PeriodicReader, SdkMeterProvider},
    propagation::TraceContextPropagator,
    trace::SdkTracerProvider,
    Resource,
};
use tracing::Span;
use tracing_opentelemetry::OpenTelemetryLayer;
use tracing_subscriber::layer::Layer;
use tracing_subscriber::{layer::SubscriberExt, util::SubscriberInitExt, EnvFilter};

use crate::config::{TelemetryConfig, TelemetryProtocol};

pub const TELEMETRY_METER_NAME: &str = "gsm";

static INIT: OnceLock<()> = OnceLock::new();
static METER_PROVIDER: OnceLock<SdkMeterProvider> = OnceLock::new();
static TELEMETRY_ENABLED: AtomicBool = AtomicBool::new(false);

pub fn init_telemetry(cfg: TelemetryConfig) -> Result<()> {
    if INIT.get().is_some() {
        TELEMETRY_ENABLED.store(cfg.exporter_enabled(), Ordering::SeqCst);
        return Ok(());
    }

    let exporters_enabled = cfg.exporter_enabled();
    TELEMETRY_ENABLED.store(exporters_enabled, Ordering::SeqCst);

    init_tracing(&cfg, exporters_enabled)?;
    if exporters_enabled {
        init_metrics(&cfg)?;
    }

    INIT.set(()).ok();
    Ok(())
}

pub fn telemetry_enabled() -> bool {
    TELEMETRY_ENABLED.load(Ordering::SeqCst)
}

pub fn with_common_fields(span: &Span, tenant: &str, chat_id: Option<&str>, msg_id: Option<&str>) {
    span.record("tenant", tracing::field::display(tenant));
    if let Some(chat_id) = chat_id {
        span.record("chat_id", tracing::field::display(chat_id));
    }
    if let Some(msg_id) = msg_id {
        span.record("msg_id", tracing::field::display(msg_id));
    }
}

fn init_tracing(cfg: &TelemetryConfig, enable_exporters: bool) -> Result<()> {
    let fmt_layer = if cfg.json_logs {
        tracing_subscriber::fmt::layer()
            .json()
            .flatten_event(true)
            .boxed()
    } else {
        tracing_subscriber::fmt::layer().boxed()
    };

    let env_filter = EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("info"));

    if enable_exporters {
        let resource = build_resource(cfg);
        let span_exporter = build_span_exporter(cfg)?;

        let tracer_provider = SdkTracerProvider::builder()
            .with_resource(resource.clone())
            .with_batch_exporter(span_exporter)
            .build();

        let tracer = tracer_provider.tracer(cfg.service_name.clone());
        global::set_tracer_provider(tracer_provider);
        global::set_text_map_propagator(TraceContextPropagator::new());

        tracing_subscriber::registry()
            .with(env_filter)
            .with(fmt_layer)
            .with(OpenTelemetryLayer::new(tracer))
            .try_init()
            .ok();
    } else {
        tracing_subscriber::registry()
            .with(env_filter)
            .with(fmt_layer)
            .try_init()
            .ok();
    }

    Ok(())
}

fn init_metrics(cfg: &TelemetryConfig) -> Result<()> {
    if METER_PROVIDER.get().is_some() {
        return Ok(());
    }

    let metric_exporter = build_metric_exporter(cfg)?;
    let reader = PeriodicReader::builder(metric_exporter)
        .with_interval(Duration::from_secs(15))
        .build();

    let provider = SdkMeterProvider::builder()
        .with_resource(build_resource(cfg))
        .with_reader(reader)
        .build();
    global::set_meter_provider(provider.clone());
    METER_PROVIDER.set(provider).ok();

    Ok(())
}

fn build_span_exporter(
    cfg: &TelemetryConfig,
) -> Result<SpanExporter, opentelemetry_otlp::ExporterBuildError> {
    match cfg.protocol {
        TelemetryProtocol::Grpc => {
            let mut builder = SpanExporter::builder().with_tonic();
            builder.export_config().endpoint = Some(cfg.endpoint.clone());
            builder.build()
        }
        TelemetryProtocol::HttpProtobuf => {
            let mut builder = SpanExporter::builder().with_http();
            builder.export_config().endpoint = Some(cfg.endpoint.clone());
            builder.build()
        }
    }
}

fn build_metric_exporter(
    cfg: &TelemetryConfig,
) -> Result<MetricExporter, opentelemetry_otlp::ExporterBuildError> {
    match cfg.protocol {
        TelemetryProtocol::Grpc => {
            let mut builder = MetricExporter::builder().with_tonic();
            builder.export_config().endpoint = Some(cfg.endpoint.clone());
            builder.build()
        }
        TelemetryProtocol::HttpProtobuf => {
            let mut builder = MetricExporter::builder().with_http();
            builder.export_config().endpoint = Some(cfg.endpoint.clone());
            builder.build()
        }
    }
}

fn build_resource(cfg: &TelemetryConfig) -> Resource {
    Resource::builder_empty()
        .with_service_name(cfg.service_name.clone())
        .with_attributes([
            KeyValue::new("service.version", cfg.service_version.clone()),
            KeyValue::new("deployment.environment", cfg.environment.clone()),
        ])
        .build()
}
