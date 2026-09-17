//! # ngy-utils-tracing
//!
//! A logging initialization utility library built on [`tracing`], which automatically selects the
//! log output based on the "application mode" ([`AppMode`]):
//!
//! - `Development`: Console logging (pretty format, with file name, line number and thread ids)
//! - `Test`: Console (pretty) + File (JSON)
//! - `Production`: File logging (JSON; one file per day, named after the date in the configured
//!   offset, see [`InitOptions::time_offset`])
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
mod retention;
mod rolling_file;
mod test_tracing;
mod timestamp;

pub use app_mode::AppMode;
pub use console_tracing::{build_debug_filter, console_tracing};
pub use file_tracing::{
    DEFAULT_LOG_DIR, DEFAULT_LOG_PREFIX, DEFAULT_MAX_LOG_FILES, build_file_filter, file_tracing,
    resolve_log_dir, resolve_log_prefix, resolve_max_log_files,
};
pub use init::{InitOptions, InitResult, get_current_mode, init};
pub use retention::cleanup_old_logs;
pub use test_tracing::test_tracing;

/// Build an environment variable name for `env_prefix`: `("NGY_", "LOG_DIR")` → `"NGY_LOG_DIR"`.
///
/// Every variable this crate reads goes through this helper except `RUST_LOG`, which keeps its
/// unprefixed name because the whole Rust logging ecosystem shares it.
pub(crate) fn env_var_name(env_prefix: &str, name: &str) -> String {
    format!("{env_prefix}{name}")
}

/// Whether [`install_subscriber`] has installed our subscriber in this process.
///
/// Warning paths that can run either side of installation (the file writer, for one) need to know
/// whether a `tracing` record reaches anybody: with a subscriber installed they use
/// `tracing::warn!`, so the message lands in the log file `Production` mode relies on, and before
/// that they fall back to stderr instead of being dropped silently.
pub(crate) static SUBSCRIBER_INSTALLED: std::sync::atomic::AtomicBool =
    std::sync::atomic::AtomicBool::new(false);

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

    // From here on a `tracing` record reaches the subscriber, so warning paths may switch from
    // stderr to `tracing::warn!` (see `SUBSCRIBER_INSTALLED`).
    SUBSCRIBER_INSTALLED.store(true, std::sync::atomic::Ordering::Relaxed);

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

/// Report a non-fatal problem in a code path that must not fail its caller.
///
/// Once [`install_subscriber`] has installed the subscriber the message goes through `tracing`,
/// because in `Production` mode the log file is the only output and a warning printed to stderr
/// would never reach it. Before that — a caller that never initialized tracing, or the cleanup that
/// runs while a file layer is still being built — `tracing` would drop the record, so stderr is used
/// instead of losing the warning silently.
pub(crate) fn warn(message: std::fmt::Arguments<'_>) {
    if SUBSCRIBER_INSTALLED.load(std::sync::atomic::Ordering::Relaxed) {
        tracing::warn!("{}", message);
    } else {
        eprintln!("[WARN] {message}");
    }
}

/// Default log level, applied when nothing more specific is configured.
///
/// [`build_debug_filter`] emits it as the fallback directive for crates that are neither listed in
/// [`InitOptions::crates`] nor named by a `RUST_LOG` directive, and [`build_file_filter`] is called
/// with it when [`file_tracing`] / [`test_tracing`] build the file layer.
pub const DEFAULT_LOG_LEVEL: &str = "info";

/// Last-resort UTC offset used for log timestamps, applied by both the console and the file layer.
///
/// An unconfigured offset follows the **machine's own time zone**; this constant only backs up a
/// machine that cannot report one. [`InitOptions::time_offset`] wins, then the `LOG_TIME_OFFSET`
/// environment variable (prefixed, see [`InitOptions::env_prefix`]), then the local zone, and this
/// value is the last resort. The accepted forms are documented on [`InitOptions::time_offset`].
pub const DEFAULT_TIME_OFFSET: &str = "UTC";

// Test-only: serialize all environment variable access across the lib test binary to avoid
// data races (unsafe) causing UB. The console_tracing, file_tracing and timestamp test modules all
// use it.
#[cfg(test)]
pub(crate) static TEST_ENV_MUTEX: std::sync::Mutex<()> = std::sync::Mutex::new(());
