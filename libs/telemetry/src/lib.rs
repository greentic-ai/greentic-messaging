//! Lightweight helpers for Greentic telemetry.
//! Provides span utilities, metric recorders, and message-context helpers
//! without owning any subscriber installation logic.

use std::cell::RefCell;
use std::path::Path;
use std::{env, fs, thread_local};

use anyhow::{Context, Result};
use greentic_types::TenantCtx;
use once_cell::sync::OnceCell;
use tracing::info;
use tracing_appender::non_blocking::WorkerGuard;
use tracing_appender::{non_blocking, rolling};
use tracing_subscriber::EnvFilter;

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

static INSTALL_GUARD: OnceCell<()> = OnceCell::new();
static DEV_GUARD: OnceCell<WorkerGuard> = OnceCell::new();

thread_local! {
    static CURRENT_TENANT: RefCell<Option<TenantCtx>> = const { RefCell::new(None) };
}

const DEV_FLAG_ENV: &str = "GREENTIC_DEV_TELEMETRY";
const DEV_LOG_DIR_ENV: &str = "GREENTIC_DEV_LOG_DIR";
const DEFAULT_LOG_DIR: &str = ".";

/// Installs a basic tracing subscriber configured from `RUST_LOG`.
pub fn install(service_name: &str) -> Result<()> {
    if INSTALL_GUARD.get().is_some() {
        return Ok(());
    }

    let dev_logging = dev_logging_enabled();
    let filter = EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("info"));
    let subscriber: Box<dyn tracing::Subscriber + Send + Sync> = if dev_logging {
        let log_path = prepare_log_destination(service_name)?;
        let file_appender = rolling::never(log_path.directory(), log_path.file_name());
        let (non_blocking_writer, guard) = non_blocking(file_appender);
        DEV_GUARD.set(guard).ok();
        Box::new(
            tracing_subscriber::fmt()
                .with_env_filter(filter.clone())
                .with_writer(non_blocking_writer)
                .finish(),
        )
    } else {
        Box::new(
            tracing_subscriber::fmt()
                .with_env_filter(filter.clone())
                .finish(),
        )
    };

    tracing::subscriber::set_global_default(subscriber)
        .with_context(|| "failed to install telemetry subscriber")?;

    INSTALL_GUARD.set(()).ok();
    if dev_logging {
        let log_file = format!("{service_name}.log");
        let dir = current_log_dir();
        info!(
            service = service_name,
            path = %Path::new(&dir).join(&log_file).display(),
            "dev telemetry logging enabled"
        );
    } else {
        info!(service = service_name, "telemetry initialized");
    }
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

fn dev_logging_enabled() -> bool {
    env::var(DEV_FLAG_ENV)
        .ok()
        .map(|value| parse_truthy(&value))
        .unwrap_or(false)
}

fn current_log_dir() -> String {
    env::var(DEV_LOG_DIR_ENV).unwrap_or_else(|_| DEFAULT_LOG_DIR.to_string())
}

struct LogPath {
    directory: String,
    file_name: String,
}

impl LogPath {
    fn directory(&self) -> &str {
        &self.directory
    }

    fn file_name(&self) -> &str {
        &self.file_name
    }
}

fn prepare_log_destination(service_name: &str) -> Result<LogPath> {
    let dir = current_log_dir();
    if dir != "." {
        fs::create_dir_all(&dir)
            .with_context(|| format!("failed to create log directory {}", dir))?;
    }
    Ok(LogPath {
        directory: dir,
        file_name: format!("{service_name}.log"),
    })
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
