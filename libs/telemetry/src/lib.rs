//! Lightweight helpers for Greentic telemetry.
//! Provides span utilities, metric recorders, and message-context helpers
//! without owning any subscriber installation logic.

use std::cell::RefCell;
use std::thread_local;

use anyhow::{Context, Result};
use greentic_types::TenantCtx;
use once_cell::sync::OnceCell;
use tracing::info;
use tracing_subscriber::EnvFilter;

mod context;
mod metrics;

pub use context::{MessageContext, TelemetryLabels};
pub use metrics::{
    record_counter, record_gauge, record_histogram, telemetry_enabled, with_common_fields,
};

static INSTALL_GUARD: OnceCell<()> = OnceCell::new();

#[allow(clippy::missing_const_for_thread_local)]
thread_local! {
    static CURRENT_TENANT: RefCell<Option<TenantCtx>> = const { RefCell::new(None) };
}

/// Installs a basic tracing subscriber configured from `RUST_LOG`.
pub fn install(service_name: &str) -> Result<()> {
    if INSTALL_GUARD.get().is_some() {
        return Ok(());
    }

    let filter = EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("info"));
    let subscriber = tracing_subscriber::fmt().with_env_filter(filter).finish();

    tracing::subscriber::set_global_default(subscriber)
        .with_context(|| "failed to install telemetry subscriber")?;

    INSTALL_GUARD.set(()).ok();
    info!(service = service_name, "telemetry initialized");
    Ok(())
}

/// Stores the `TenantCtx` for the current thread.
pub fn set_current_tenant_ctx(ctx: TenantCtx) {
    CURRENT_TENANT.with(|slot| {
        *slot.borrow_mut() = Some(ctx);
    });
}

/// Retrieves the last `TenantCtx` recorded on the current thread.
pub fn current_tenant_ctx() -> Option<TenantCtx> {
    CURRENT_TENANT.with(|slot| slot.borrow().clone())
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

#[cfg(test)]
pub(crate) mod test_support {
    use std::sync::{Mutex, MutexGuard, OnceLock};

    #[allow(dead_code)]
    pub fn metrics_lock() -> &'static Mutex<()> {
        static LOCK: OnceLock<Mutex<()>> = OnceLock::new();
        LOCK.get_or_init(|| Mutex::new(()))
    }

    #[allow(dead_code)]
    pub fn metrics_guard() -> MutexGuard<'static, ()> {
        metrics_lock().lock().unwrap_or_else(|err| err.into_inner())
    }
}
