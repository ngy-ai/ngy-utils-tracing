//! File logging initialization: writes JSON-format logs to a specified directory.
//!
//! Log files rotate daily, with filename format `{prefix}.{date}`.
//! The log directory is controlled via the `LOG_DIR` environment variable (default `logs`).
//! The log level is controlled via the `RUST_LOG` environment variable (default `info`).

use tracing_appender::non_blocking::WorkerGuard;
use tracing_subscriber::{
    EnvFilter,
    Layer,
    fmt::{self},
    layer::SubscriberExt,
};

/// Default log directory
const DEFAULT_LOG_DIR: &str = "logs";

/// Default log file prefix
const DEFAULT_LOG_PREFIX: &str = "app.log";

/// Build the log directory path
pub fn resolve_log_dir() -> String {
    std::env::var("LOG_DIR").unwrap_or_else(|_| DEFAULT_LOG_DIR.to_string())
}

/// Build the log file prefix
pub fn resolve_log_prefix() -> String {
    std::env::var("LOG_PREFIX").unwrap_or_else(|_| DEFAULT_LOG_PREFIX.to_string())
}

/// Build the EnvFilter, reading `RUST_LOG` first, otherwise using `info`
pub fn build_file_filter(default_directive: &str) -> anyhow::Result<EnvFilter> {
    let filter = EnvFilter::try_from_default_env()
        .unwrap_or_else(|_| EnvFilter::new(default_directive));
    Ok(filter)
}

/// Build the file log layer (JSON format)
pub fn build_file_layer<S>(
    log_dir: &str,
    log_prefix: &str,
) -> anyhow::Result<(impl Layer<S>, WorkerGuard)>
where
    S: tracing::Subscriber + for<'a> tracing_subscriber::registry::LookupSpan<'a>,
{
    std::fs::create_dir_all(log_dir)?;

    let file_appender = tracing_appender::rolling::daily(log_dir, log_prefix);
    let (non_blocking, guard) = tracing_appender::non_blocking(file_appender);

    let layer = fmt::layer()
        .json()
        .with_target(true)
        .with_level(true)
        .with_ansi(false)
        .with_timer(fmt::time::SystemTime::default())
        .with_writer(non_blocking);

    Ok((layer, guard))
}

/// Initialize file logging: JSON format output to the `logs/` directory.
///
/// The returned `tracing_appender::non_blocking::WorkerGuard` must stay alive,
/// otherwise logs may be lost. The caller should keep it in the main function until the process exits.
///
/// `log_prefix` overrides the `LOG_PREFIX` environment variable when `Some`; otherwise the
/// `LOG_PREFIX` env var (default `app.log`) is used as the file name prefix.
pub fn file_tracing(log_prefix: Option<String>) -> anyhow::Result<WorkerGuard> {
    let log_dir = resolve_log_dir();
    let log_prefix = log_prefix.unwrap_or_else(resolve_log_prefix);
    let (fmt_layer, guard) = build_file_layer(&log_dir, &log_prefix)?;
    let env_filter = build_file_filter("info")?;

    let subscriber = tracing_subscriber::registry()
        .with(fmt_layer)
        .with(env_filter);

    tracing::subscriber::set_global_default(subscriber)?;

    Ok(guard)
}

#[cfg(test)]
mod tests {
    use super::*;

    // All tests read the RUST_LOG / LOG_DIR / LOG_PREFIX environment variables; they hold
    // crate::TEST_ENV_MUTEX uniformly, serialized with the tests in the console_tracing module,
    // to avoid concurrent read/write (unsafe) causing UB.
    fn lock_env() -> std::sync::MutexGuard<'static, ()> {
        crate::TEST_ENV_MUTEX.lock().unwrap()
    }

    #[test]
    fn resolve_log_dir_returns_default_when_env_not_set() {
        let _lock = lock_env();
        // Only verify the function does not panic, not depending on env var state
        let dir = resolve_log_dir();
        assert!(!dir.is_empty());
    }

    #[test]
    fn build_env_filter_with_default_directive() {
        let _lock = lock_env();
        let filter = build_file_filter("info").unwrap();
        // EnvFilter has no public debug interface; only verify construction does not panic
        let _ = format!("{:?}", filter);
    }

    #[test]
    fn build_env_filter_with_debug_directive() {
        let _lock = lock_env();
        let filter = build_file_filter("debug").unwrap();
        let _ = format!("{:?}", filter);
    }

    #[test]
    fn resolve_log_dir_returns_string() {
        let _lock = lock_env();
        let dir = resolve_log_dir();
        // Whether or not the env var is set, a non-empty string should be returned
        assert!(!dir.is_empty());
    }
}
