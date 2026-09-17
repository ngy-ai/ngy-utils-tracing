//! File logging initialization: writes JSON-format logs to a specified directory.
//!
//! Log files rotate daily, with filename format `{prefix}.{date}`; on initialization the oldest
//! files beyond the retention limit are deleted (the policy itself lives in [`crate::retention`]),
//! leaving room for the file created for the current day, so the directory never holds more than
//! `LOG_MAX_FILES` files. Every setting and its default is documented once in
//! [`InitOptions`](crate::InitOptions); this module also resolves them from the environment.
//!
//! Timestamps are RFC 3339 with the offset resolved in [`crate::timestamp`], shared with the
//! console layer so both report the same wall-clock time.

use crate::retention::{LogRetentionHandle, cleanup_old_logs, spawn_retention_task};
use time::UtcOffset;
use tracing_appender::non_blocking::WorkerGuard;
use tracing_subscriber::{
    EnvFilter, Layer,
    fmt::{self},
    layer::SubscriberExt,
};

/// Default log directory, used when `LOG_DIR` is unset.
pub const DEFAULT_LOG_DIR: &str = "logs";

/// Default log file prefix, used when `LOG_PREFIX` is unset.
pub const DEFAULT_LOG_PREFIX: &str = "app.log";

/// Default number of retained log files, used when `LOG_MAX_FILES` is unset; older files beyond
/// this are deleted on init.
pub const DEFAULT_MAX_LOG_FILES: usize = 7;

/// Build the log directory path
///
/// Reads the prefixed `LOG_DIR` variable (see [`crate::InitOptions::env_prefix`]), falling back to
/// [`DEFAULT_LOG_DIR`] when it is unset.
pub fn resolve_log_dir(env_prefix: &str) -> String {
    let name = crate::env_var_name(env_prefix, "LOG_DIR");
    std::env::var(name).unwrap_or_else(|_| DEFAULT_LOG_DIR.to_string())
}

/// Build the log file prefix
///
/// Reads the prefixed `LOG_PREFIX` variable, falling back to [`DEFAULT_LOG_PREFIX`] when it is
/// unset.
pub fn resolve_log_prefix(env_prefix: &str) -> String {
    let name = crate::env_var_name(env_prefix, "LOG_PREFIX");
    std::env::var(name).unwrap_or_else(|_| DEFAULT_LOG_PREFIX.to_string())
}

/// Resolve the maximum number of retained log files.
///
/// Reads the prefixed `LOG_MAX_FILES` variable; falls back to [`DEFAULT_MAX_LOG_FILES`] when it is
/// unset or when the value is not a positive integer.
pub fn resolve_max_log_files(env_prefix: &str) -> usize {
    let name = crate::env_var_name(env_prefix, "LOG_MAX_FILES");
    std::env::var(name)
        .ok()
        .and_then(|v| v.parse::<usize>().ok())
        .filter(|&n| n > 0)
        .unwrap_or(DEFAULT_MAX_LOG_FILES)
}

/// Start a background task that periodically enforces the log-file retention limit.
///
/// Unlike the one-shot cleanup at initialization, this keeps the log directory bounded for
/// processes that run across many daily rotations. The task itself is implemented in the
/// `retention` module; this function only resolves the settings it needs.
///
/// - `env_prefix`: prefix used for the `LOG_DIR` / `LOG_PREFIX` / `LOG_MAX_FILES` lookups; see
///   [`crate::InitOptions::env_prefix`].
/// - `log_dir`: overrides the prefixed `LOG_DIR` when `Some`; otherwise [`DEFAULT_LOG_DIR`] is used.
/// - `log_prefix`: overrides the prefixed `LOG_PREFIX` when `Some`; otherwise [`DEFAULT_LOG_PREFIX`]
///   is used.
/// - `max_log_files`: overrides the prefixed `LOG_MAX_FILES` when `Some`; otherwise
///   [`DEFAULT_MAX_LOG_FILES`] is used. Values below `1` are treated as `1`, so the file currently
///   being written is never removed.
/// - `interval`: how often cleanup runs. Use [`crate::DEFAULT_RETENTION_INTERVAL`] for the default.
///
/// # Example
///
/// ```no_run
/// use ngy_utils_tracing::{start_log_retention, DEFAULT_RETENTION_INTERVAL};
///
/// // Keep the handle alive (like the WorkerGuard) for the process lifetime.
/// let _retention =
///     start_log_retention("NGY_", None, None, None, DEFAULT_RETENTION_INTERVAL).unwrap();
/// ```
///
/// # Errors
///
/// Returns an error if the worker thread cannot be spawned.
pub fn start_log_retention(
    env_prefix: &str,
    log_dir: Option<String>,
    log_prefix: Option<String>,
    max_log_files: Option<usize>,
    interval: std::time::Duration,
) -> anyhow::Result<LogRetentionHandle> {
    let log_dir = log_dir.unwrap_or_else(|| resolve_log_dir(env_prefix));
    let log_prefix = log_prefix.unwrap_or_else(|| resolve_log_prefix(env_prefix));
    let max_files = max_log_files.unwrap_or_else(|| resolve_max_log_files(env_prefix));

    spawn_retention_task(log_dir, log_prefix, max_files, interval)
}

/// Build the `EnvFilter`, reading `RUST_LOG` first and otherwise falling back to
/// `default_directive` (callers pass [`crate::DEFAULT_LOG_LEVEL`]).
pub fn build_file_filter(default_directive: &str) -> anyhow::Result<EnvFilter> {
    let filter =
        EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new(default_directive));
    Ok(filter)
}

/// Build the file log layer (JSON format) using the given timestamp offset
///
/// Timestamps are written as RFC 3339 with an explicit offset
/// (e.g. `2026-09-17T19:03:04.123456+08:00`), which stays sortable and unambiguous for log
/// collectors. Level and target are included by default; ANSI is disabled because the output does
/// not go to a terminal.
pub(crate) fn build_file_layer<S>(
    log_dir: &str,
    log_prefix: &str,
    max_files: usize,
    time_offset: UtcOffset,
) -> anyhow::Result<(impl Layer<S>, WorkerGuard)>
where
    S: tracing::Subscriber + for<'a> tracing_subscriber::registry::LookupSpan<'a>,
{
    std::fs::create_dir_all(log_dir)?;

    // Enforce retention before opening the new appender, leaving one free slot for the file
    // created for the current day so the directory never holds more than `max_files` files.
    cleanup_old_logs(log_dir, log_prefix, max_files.saturating_sub(1))?;

    let file_appender = tracing_appender::rolling::daily(log_dir, log_prefix);
    let (non_blocking, guard) = tracing_appender::non_blocking(file_appender);

    let layer = fmt::layer()
        .json()
        .with_ansi(false)
        .with_timer(crate::timestamp::rfc3339_timer(time_offset))
        .with_writer(non_blocking);

    Ok((layer, guard))
}

/// Initialize file logging: JSON format output to the log directory.
///
/// The returned `tracing_appender::non_blocking::WorkerGuard` must stay alive,
/// otherwise logs may be lost. The caller should keep it in the main function until the process exits.
///
/// The timestamp offset comes from the prefixed `LOG_TIME_OFFSET` variable, defaulting to
/// [`crate::DEFAULT_TIME_OFFSET`].
///
/// - `env_prefix`: prefix for every variable this function reads; see
///   [`InitOptions::env_prefix`](crate::InitOptions::env_prefix).
/// - `log_dir`: overrides the prefixed `LOG_DIR` variable when `Some`; otherwise
///   [`DEFAULT_LOG_DIR`] is used.
/// - `log_prefix`: overrides the prefixed `LOG_PREFIX` variable when `Some`; otherwise
///   [`DEFAULT_LOG_PREFIX`] is used as the file name prefix.
/// - `max_log_files`: overrides the prefixed `LOG_MAX_FILES` variable when `Some`;
///   otherwise [`DEFAULT_MAX_LOG_FILES`] is used as the retention limit.
pub fn file_tracing(
    env_prefix: &str,
    log_dir: Option<String>,
    log_prefix: Option<String>,
    max_log_files: Option<usize>,
) -> anyhow::Result<WorkerGuard> {
    file_tracing_with_offset(env_prefix, log_dir, log_prefix, max_log_files, None)
}

/// [`file_tracing`] with an explicit timestamp offset override; used by [`crate::init`] so that
/// [`InitOptions::time_offset`](crate::InitOptions::time_offset) reaches the layer.
pub(crate) fn file_tracing_with_offset(
    env_prefix: &str,
    log_dir: Option<String>,
    log_prefix: Option<String>,
    max_log_files: Option<usize>,
    time_offset: Option<&str>,
) -> anyhow::Result<WorkerGuard> {
    let log_dir = log_dir.unwrap_or_else(|| resolve_log_dir(env_prefix));
    let log_prefix = log_prefix.unwrap_or_else(|| resolve_log_prefix(env_prefix));
    let max_files = max_log_files.unwrap_or_else(|| resolve_max_log_files(env_prefix));
    let time_offset = crate::timestamp::resolve_time_offset(env_prefix, time_offset);

    let (fmt_layer, guard) = build_file_layer(&log_dir, &log_prefix, max_files, time_offset)?;
    let env_filter = build_file_filter(crate::DEFAULT_LOG_LEVEL)?;

    // `crate::install_subscriber` also installs the `log` compatibility layer and syncs its max
    // level, which plain `set_global_default` does not; without it every `log::info!` record from a
    // dependency would be dropped in `Production` mode.
    crate::install_subscriber(
        tracing_subscriber::registry().with(fmt_layer.with_filter(env_filter)),
    )?;

    Ok(guard)
}

#[cfg(test)]
mod tests {
    use super::*;

    // These tests read RUST_LOG / the prefixed LOG_* variables; they hold crate::TEST_ENV_MUTEX
    // uniformly, serialized with the tests in the console_tracing and retention modules, to avoid
    // concurrent read/write (unsafe) causing UB.
    fn lock_env() -> std::sync::MutexGuard<'static, ()> {
        crate::TEST_ENV_MUTEX.lock().unwrap()
    }

    /// Prefix used by the tests so they never touch a variable name a real deployment could use.
    const TEST_PREFIX: &str = "NGY_TEST_";

    /// Name of a prefixed variable, e.g. `env_name("LOG_DIR")` → `NGY_TEST_LOG_DIR`.
    fn env_name(suffix: &str) -> String {
        crate::env_var_name(TEST_PREFIX, suffix)
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
    fn resolve_log_dir_reads_the_prefixed_variable_only() {
        let _lock = lock_env();
        let name = env_name("LOG_DIR");
        // SAFETY: unsafe in Rust 2024; runtime tests hold TEST_ENV_MUTEX for all env access.
        unsafe { std::env::remove_var(&name) };
        assert_eq!(resolve_log_dir(TEST_PREFIX), DEFAULT_LOG_DIR);

        // SAFETY: Same as above.
        unsafe { std::env::set_var(&name, "custom-logs") };
        assert_eq!(resolve_log_dir(TEST_PREFIX), "custom-logs");

        // Another program owning the unprefixed name must not influence us.
        // SAFETY: Same as above.
        unsafe { std::env::set_var("LOG_DIR", "someone-elses-logs") };
        assert_eq!(resolve_log_dir(TEST_PREFIX), "custom-logs");

        // SAFETY: Same as above; restore the environment this test changed.
        unsafe {
            std::env::remove_var(&name);
            std::env::remove_var("LOG_DIR");
        }
    }

    #[test]
    fn resolve_log_prefix_reads_the_prefixed_variable() {
        let _lock = lock_env();
        let name = env_name("LOG_PREFIX");
        // SAFETY: unsafe in Rust 2024; runtime tests hold TEST_ENV_MUTEX for all env access.
        unsafe { std::env::remove_var(&name) };
        assert_eq!(resolve_log_prefix(TEST_PREFIX), DEFAULT_LOG_PREFIX);

        // SAFETY: Same as above.
        unsafe { std::env::set_var(&name, "svc.log") };
        assert_eq!(resolve_log_prefix(TEST_PREFIX), "svc.log");

        // SAFETY: Same as above; restore the environment this test changed.
        unsafe { std::env::remove_var(&name) };
    }

    #[test]
    fn resolve_max_log_files_rejects_non_positive_values() {
        let _lock = lock_env();
        let name = env_name("LOG_MAX_FILES");
        // SAFETY: unsafe in Rust 2024; runtime tests hold TEST_ENV_MUTEX for all env access.
        unsafe { std::env::remove_var(&name) };
        assert_eq!(resolve_max_log_files(TEST_PREFIX), DEFAULT_MAX_LOG_FILES);

        // SAFETY: Same as above.
        unsafe { std::env::set_var(&name, "3") };
        assert_eq!(resolve_max_log_files(TEST_PREFIX), 3);

        // SAFETY: Same as above.
        unsafe { std::env::set_var(&name, "0") };
        assert_eq!(resolve_max_log_files(TEST_PREFIX), DEFAULT_MAX_LOG_FILES);

        // SAFETY: Same as above; restore the environment this test changed.
        unsafe { std::env::remove_var(&name) };
    }
}
