//! # ngy-utils-tracing
//!
//! A logging initialization utility library built on [`tracing`], which automatically selects the
//! log output based on the "application mode" ([`AppMode`]):
//!
//! - `Development`: Console logging (pretty format, with file name, line number and thread ids)
//! - `Test`: Console (pretty) + File (JSON)
//! - `Production`: File logging (JSON, daily rotation)
//!
//! [`init`] performs one-time initialization of dotenv and tracing; repeated calls within the
//! process return a clear error. After initialization, use [`get_current_mode`] to query the
//! current mode.
//!
//! Every environment variable (log directory, application mode, timestamp offset, …) is read with
//! the required prefix passed to [`InitOptions::new`], so an unrelated program that happens to use
//! `LOG_DIR` or `APP_MODE` cannot change this crate's behaviour. `RUST_LOG` is the one exception:
//! it keeps its unprefixed name because the whole Rust logging ecosystem shares it.
//!
//! See the [README](https://crates.io/crates/ngy-utils-tracing) for mode descriptions and the full API.

use tracing_log::AsLog;

mod app_mode;
mod console_tracing;
mod file_tracing;
mod init;
mod test_tracing;
mod timestamp;

pub use app_mode::AppMode;
pub use console_tracing::{build_debug_filter, console_tracing};
pub use file_tracing::{
    DEFAULT_LOG_DIR, DEFAULT_LOG_PREFIX, DEFAULT_MAX_LOG_FILES, DEFAULT_RETENTION_INTERVAL,
    LogRetentionHandle, build_file_filter, cleanup_old_logs, file_tracing, resolve_log_dir,
    resolve_log_prefix, resolve_max_log_files, resolve_retention_interval, start_log_retention,
};
pub use init::{InitOptions, InitResult, get_current_mode, init};
pub use test_tracing::test_tracing;

/// Build an environment variable name for `env_prefix`: `("NGY_", "LOG_DIR")` → `"NGY_LOG_DIR"`.
///
/// Every variable this crate reads goes through this helper except `RUST_LOG`, which keeps its
/// unprefixed name because the whole Rust logging ecosystem shares it.
pub(crate) fn env_var_name(env_prefix: &str, name: &str) -> String {
    format!("{env_prefix}{name}")
}

/// Install `subscriber` as the process-wide default and, best effort, bridge `log` records into it.
///
/// A global subscriber that is already installed is a hard error: without ours, none of the
/// logging configured here takes effect, so the caller must know. The `log` compatibility layer is
/// different — an application may legitimately have installed a `log` logger (`env_logger`, …)
/// before calling [`init`], so that case is reported on stderr and skipped rather than failing the
/// whole initialization. This is why `SubscriberInitExt::try_init` is not used: it treats both
/// cases as fatal.
pub(crate) fn install_subscriber<S>(subscriber: S) -> anyhow::Result<()>
where
    S: tracing::Subscriber + Send + Sync + 'static,
{
    tracing::subscriber::set_global_default(subscriber).map_err(|err| {
        anyhow::anyhow!(
            "a global tracing subscriber is already installed, so the logging configuration of \
             this crate cannot be applied: {err}"
        )
    })?;

    // Same numeric level hint `try_init` would pass, so `log` records that the subscriber filters
    // out are skipped as early as possible.
    let max_level = tracing::level_filters::LevelFilter::current().as_log();
    if let Err(err) = tracing_log::LogTracer::builder()
        .with_max_level(max_level)
        .init()
    {
        eprintln!(
            "[WARN] a `log` logger is already installed, so `log` records will not be converted \
             into tracing events: {err}"
        );
    }

    Ok(())
}

/// Default log level, applied when nothing more specific is configured.
///
/// [`build_debug_filter`] emits it as the fallback directive for crates that are neither listed in
/// [`InitOptions::crates`] nor named by a `RUST_LOG` directive, and [`build_file_filter`] is called
/// with it when [`file_tracing`] / [`test_tracing`] build the file layer.
pub const DEFAULT_LOG_LEVEL: &str = "info";

/// Default UTC offset used for log timestamps, applied by both the console and the file layer.
///
/// Overridden by [`InitOptions::time_offset`] and then by the `LOG_TIME_OFFSET` environment
/// variable (prefixed, see [`InitOptions::env_prefix`]); the accepted forms are documented on
/// [`InitOptions::time_offset`].
pub const DEFAULT_TIME_OFFSET: &str = "+08:00";

// Test-only: serialize all environment variable access across the lib test binary to avoid
// data races (unsafe) causing UB. Tests in both console_tracing and file_tracing modules use it.
#[cfg(test)]
pub(crate) static TEST_ENV_MUTEX: std::sync::Mutex<()> = std::sync::Mutex::new(());
