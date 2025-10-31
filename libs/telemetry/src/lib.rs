//! Lightweight facade around tracing + OpenTelemetry setup.
//!
//! ```no_run
//! use gsm_telemetry::{init_telemetry, TelemetryConfig};
//!
//! # fn main() -> anyhow::Result<()> {
//! let cfg = TelemetryConfig {
//!     service_name: "example-service".into(),
//!     service_version: "0.1.0".into(),
//!     enabled: false,
//!     endpoint: String::new(),
//!     protocol: gsm_telemetry::TelemetryProtocol::Grpc,
//!     json_logs: true,
//!     environment: "local".into(),
//! };
//! init_telemetry(cfg)?;
//! tracing::info!("telemetry configured");
//! Ok(())
//! # }
//! ```

mod config;
mod context;
mod metrics;
mod tracing_init;

pub use config::{TelemetryConfig, TelemetryProtocol};
pub use context::{MessageContext, TelemetryLabels};
pub use metrics::{record_counter, record_gauge, record_histogram};
pub use tracing_init::{
    init_telemetry, telemetry_enabled, with_common_fields, TELEMETRY_METER_NAME,
};

#[cfg(test)]
pub(crate) mod test_support {
    use std::sync::{Mutex, MutexGuard, OnceLock};

    pub fn env_lock() -> &'static Mutex<()> {
        static LOCK: OnceLock<Mutex<()>> = OnceLock::new();
        LOCK.get_or_init(|| Mutex::new(()))
    }

    pub fn env_guard() -> MutexGuard<'static, ()> {
        env_lock().lock().unwrap_or_else(|err| err.into_inner())
    }
}

#[macro_export]
macro_rules! counter {
    ($name:expr, $value:expr, $labels:expr) => {{
        $crate::metrics::record_counter($name, $value, $labels)
    }};
}

#[macro_export]
macro_rules! histogram {
    ($name:expr, $value:expr, $labels:expr) => {{
        $crate::metrics::record_histogram($name, $value, $labels)
    }};
}

#[macro_export]
macro_rules! gauge {
    ($name:expr, $value:expr, $labels:expr) => {{
        $crate::metrics::record_gauge($name, $value, $labels)
    }};
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_support::env_guard;

    #[test]
    fn config_defaults_disabled() {
        let _guard = env_guard();
        std::env::remove_var("ENABLE_OTEL");
        std::env::remove_var("OTEL_EXPORTER_OTLP_ENDPOINT");
        let cfg = TelemetryConfig::from_env("test-service", "0.0.1");
        assert!(!cfg.enabled);
        assert!(cfg.endpoint.is_empty());
        assert!(cfg.json_logs);
    }

    #[test]
    fn config_enabled_when_flag_and_endpoint() {
        let _guard = env_guard();
        std::env::set_var("ENABLE_OTEL", "true");
        std::env::set_var("OTEL_EXPORTER_OTLP_ENDPOINT", "http://localhost:4317");
        std::env::set_var("LOG_FORMAT", "text");
        let cfg = TelemetryConfig::from_env("svc", "1.2.3");
        assert!(cfg.enabled);
        assert_eq!(cfg.endpoint, "http://localhost:4317");
        assert!(!cfg.json_logs);
        std::env::remove_var("ENABLE_OTEL");
        std::env::remove_var("OTEL_EXPORTER_OTLP_ENDPOINT");
        std::env::remove_var("LOG_FORMAT");
    }

    #[test]
    fn init_noop_when_disabled() {
        let _guard = env_guard();
        std::env::remove_var("ENABLE_OTEL");
        std::env::remove_var("OTEL_EXPORTER_OTLP_ENDPOINT");
        let cfg = TelemetryConfig::from_env("svc", "1.0.0");
        init_telemetry(cfg).expect("init should succeed");
        assert!(!crate::tracing_init::telemetry_enabled());
    }
}
