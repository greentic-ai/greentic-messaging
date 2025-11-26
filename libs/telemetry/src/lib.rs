//! Lightweight helpers for Greentic telemetry.
//! Provides span utilities, metric recorders, and message-context helpers
//! built on top of the shared `greentic-telemetry` crate.

use anyhow::Result;
use greentic_telemetry::{TelemetryConfig, TelemetryCtx};
use greentic_types::TenantCtx;
use std::env;

mod auth;
mod context;
mod metrics;

pub use auth::{
    AuthRenderMode, record_auth_card_clicked, record_auth_card_render,
    record_auth_card_render_with_labels,
};
pub use context::{MessageContext, TelemetryLabels};
pub use metrics::{
    record_counter, record_gauge, record_histogram, telemetry_enabled, with_common_fields,
};

const DEV_FLAG_ENV: &str = "GREENTIC_DEV_TELEMETRY";
const GREENTIC_FMT_ENV: &str = "GT_TELEMETRY_FMT";

/// Installs the shared telemetry subscriber configured from `RUST_LOG`.
pub fn install(service_name: &str) -> Result<()> {
    if dev_logging_enabled() && env::var_os(GREENTIC_FMT_ENV).is_none() {
        // The shared greentic-telemetry crate enables structured stdout logs when
        // `GT_TELEMETRY_FMT=1`. Preserve the old GREENTIC_DEV_TELEMETRY switch by
        // toggling the new env var automatically.
        unsafe {
            // SAFETY: the process intentionally sets a process-wide env var before
            // installing the telemetry subscriber, mirroring previous behaviour.
            env::set_var(GREENTIC_FMT_ENV, "1");
        }
    }

    greentic_telemetry::init_telemetry(TelemetryConfig {
        service_name: service_name.to_string(),
    })
}

/// Stores the `TenantCtx` on the current Tokio task.
pub fn set_current_tenant_ctx(ctx: TenantCtx) {
    let mut telemetry_ctx = TelemetryCtx::new(ctx.tenant.as_str().to_string());
    if let Some(session) = ctx.session_id() {
        telemetry_ctx = telemetry_ctx.with_session(session.to_string());
    }
    if let Some(flow) = ctx.flow_id() {
        telemetry_ctx = telemetry_ctx.with_flow(flow.to_string());
    }
    if let Some(node) = ctx.node_id() {
        telemetry_ctx = telemetry_ctx.with_node(node.to_string());
    }
    if let Some(provider) = ctx.provider_id.as_ref() {
        telemetry_ctx = telemetry_ctx.with_provider(provider.as_str().to_string());
    }
    greentic_telemetry::set_current_telemetry_ctx(telemetry_ctx);
}

#[macro_export]
macro_rules! counter {
    ($name:expr, $value:expr, $labels:expr) => {{ $crate::metrics::record_counter($name, $value, $labels) }};
}

#[macro_export]
macro_rules! histogram {
    ($name:expr, $value:expr, $labels:expr) => {{ $crate::metrics::record_histogram($name, $value, $labels) }};
}

#[macro_export]
macro_rules! gauge {
    ($name:expr, $value:expr, $labels:expr) => {{ $crate::metrics::record_gauge($name, $value, $labels) }};
}

fn dev_logging_enabled() -> bool {
    env::var(DEV_FLAG_ENV)
        .ok()
        .map(|value| parse_truthy(&value))
        .unwrap_or(false)
}

fn parse_truthy(value: &str) -> bool {
    matches!(
        value.trim().to_ascii_lowercase().as_str(),
        "1" | "true" | "yes" | "on"
    )
}

#[cfg(test)]
mod tests {
    use super::parse_truthy;

    #[test]
    fn truthy_variants_are_detected() {
        for value in ["1", "true", "TRUE", " yes ", "On"] {
            assert!(parse_truthy(value), "value {value:?} should be truthy");
        }
    }

    #[test]
    fn falsy_variants_are_detected() {
        for value in ["0", "false", "off", "", "nope"] {
            assert!(!parse_truthy(value), "value {value:?} should be falsy");
        }
    }
}
