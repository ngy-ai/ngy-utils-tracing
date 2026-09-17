//! File logging initialization: writes JSON-format logs to a specified directory.
//!
//! Log files rotate daily, with filename format `{prefix}.{date}`; on initialization the oldest
//! files beyond the retention limit are deleted, leaving room for the file created for the current
//! day, so the directory never holds more than `LOG_MAX_FILES` files. Every setting and its default
//! is documented once in [`InitOptions`](crate::InitOptions); the resolvers below implement exactly
//! that mapping, and the defaults are exposed as the `DEFAULT_*` constants in this module.
//!
//! Timestamps are RFC 3339 with the offset resolved in [`crate::timestamp`], shared with the
//! console layer so both report the same wall-clock time.

use std::sync::atomic::{AtomicBool, Ordering};
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

/// Default interval at which the background retention task runs [`cleanup_old_logs`].
pub const DEFAULT_RETENTION_INTERVAL: std::time::Duration = std::time::Duration::from_secs(3600);

/// Suffix of the environment variable holding the background retention interval, in seconds.
const RETENTION_INTERVAL_ENV: &str = "LOG_RETENTION_INTERVAL_SECONDS";

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

/// Resolve the background retention interval from the prefixed `LOG_RETENTION_INTERVAL_SECONDS`
/// variable.
///
/// The value is interpreted as **seconds**. Returns `None` when unset or when the value is `0`
/// (disabled), so callers fall back to running no background task unless an explicit interval is
/// provided via [`crate::InitOptions::retention_interval`].
///
/// Note: [`crate::init`] does not use this function directly; it uses
/// [`resolve_retention_interval_or_default`] so that periodic retention is enabled by default.
pub fn resolve_retention_interval(env_prefix: &str) -> Option<std::time::Duration> {
    let name = crate::env_var_name(env_prefix, RETENTION_INTERVAL_ENV);
    std::env::var(name)
        .ok()
        .and_then(|v| v.parse::<u64>().ok())
        .filter(|&secs| secs > 0)
        .map(std::time::Duration::from_secs)
}

/// Resolve the effective background retention interval used by [`crate::init`].
///
/// Unlike [`resolve_retention_interval`], this keeps periodic retention **enabled by default**:
///
/// - the prefixed `LOG_RETENTION_INTERVAL_SECONDS` set to `0` → `None` (task explicitly disabled)
/// - a positive value → that many seconds
/// - unset or invalid → [`DEFAULT_RETENTION_INTERVAL`]
pub(crate) fn resolve_retention_interval_or_default(
    env_prefix: &str,
) -> Option<std::time::Duration> {
    let name = crate::env_var_name(env_prefix, RETENTION_INTERVAL_ENV);
    match std::env::var(name) {
        Ok(value) => match value.trim().parse::<u64>() {
            Ok(0) => None,
            Ok(secs) => Some(std::time::Duration::from_secs(secs)),
            Err(_) => Some(DEFAULT_RETENTION_INTERVAL),
        },
        Err(_) => Some(DEFAULT_RETENTION_INTERVAL),
    }
}

/// A candidate log file found in the log directory: its path, the date parsed from its name
/// (`None` for a bare `{prefix}` file), and its modification time.
type LogFileEntry = (
    std::path::PathBuf,
    Option<(i32, u8, u8)>,
    std::time::SystemTime,
);

/// Extract the date embedded in a rotated log file name (`{prefix}.{YYYY-MM-DD}`).
///
/// Returns `None` when the name is not `{prefix}.{date}` or the date is invalid, so that
/// unrelated files (e.g. `app.logger`) are never treated as log files.
fn parse_file_date(file_name: &str, log_prefix: &str) -> Option<(i32, u8, u8)> {
    let suffix = file_name.strip_prefix(log_prefix)?.strip_prefix('.')?;
    let mut parts = suffix.split('-');
    let year: i32 = parts.next()?.parse().ok()?;
    let month: u8 = parts.next()?.parse().ok()?;
    let day: u8 = parts.next()?.parse().ok()?;
    if parts.next().is_some() || !(1..=12).contains(&month) || !(1..=31).contains(&day) {
        return None;
    }
    Some((year, month, day))
}

/// Delete old log files in `log_dir`, keeping only the most recent `max_files`.
///
/// Only daily-rotation outputs are matched: a file counts as a log when its name is exactly
/// `log_prefix` or has the form `{log_prefix}.{YYYY-MM-DD}` (e.g. `app.log.2026-09-16`).
/// Unrelated files that merely share the prefix (e.g. `app.logger` or `app.log.backup`) are
/// left untouched.
///
/// Files are ordered by the date embedded in their name, which stays correct even when a
/// file's modification time has been altered by a copy or restore. Names without a parseable
/// date sort as the oldest, and modification time breaks ties.
///
/// `max_files == 0` keeps nothing and deletes every matching file; callers must make sure this
/// does not target a file that is currently open for writing (the startup path runs before the
/// new file is created, which is safe).
///
/// This runs at initialization. To enforce retention continuously (e.g. across
/// many daily rotations within a long-running process), call it periodically
/// from a background task.
///
/// # Errors
///
/// Returns an error if the log directory cannot be read.
pub fn cleanup_old_logs(log_dir: &str, log_prefix: &str, max_files: usize) -> anyhow::Result<()> {
    let mut files: Vec<LogFileEntry> = std::fs::read_dir(log_dir)?
        .filter_map(|e| e.ok())
        .filter_map(|entry| {
            let path = entry.path();
            let name = path.file_name()?.to_string_lossy().to_string();
            let date = parse_file_date(&name, log_prefix);
            // Accept only `log_prefix` itself or `{log_prefix}.{date}`; everything else
            // (e.g. `app.logger`) is not a rotated log file and must be preserved.
            if !path.is_file() || (name != log_prefix && date.is_none()) {
                return None;
            }
            let modified = std::fs::metadata(&path).and_then(|m| m.modified()).ok()?;
            Some((path, date, modified))
        })
        .collect();

    // Oldest first so we can drop the front of the list. Dated names sort before undated ones
    // (which are treated as the oldest); modification time only breaks ties.
    files.sort_by(
        |(_, a_date, a_mtime), (_, b_date, b_mtime)| match (a_date, b_date) {
            (Some(a), Some(b)) => a.cmp(b).then_with(|| a_mtime.cmp(b_mtime)),
            (Some(_), None) => std::cmp::Ordering::Greater,
            (None, Some(_)) => std::cmp::Ordering::Less,
            (None, None) => a_mtime.cmp(b_mtime),
        },
    );

    let excess = files.len().saturating_sub(max_files);
    for (path, _, _) in files.into_iter().take(excess) {
        // Deleting a file that is currently open for writing would be a user error
        // (the active file carries the newest date and is kept); ignore failures.
        if let Err(err) = std::fs::remove_file(&path) {
            eprintln!(
                "[WARN] failed to remove old log file {}: {}",
                path.display(),
                err
            );
        }
    }

    Ok(())
}

/// Handle for a background log-retention task started by [`start_log_retention`].
///
/// The task periodically calls [`cleanup_old_logs`] to enforce the retention limit across
/// many daily rotations within a long-running process. Dropping the handle stops the task
/// and joins the worker thread; you can also call [`LogRetentionHandle::stop`] explicitly.
///
/// Keep the handle alive for as long as the process runs (e.g. alongside the file
/// `WorkerGuard` returned by `init`/`file_tracing`).
pub struct LogRetentionHandle {
    shutdown: std::sync::Arc<AtomicBool>,
    join: Option<std::thread::JoinHandle<()>>,
}

impl LogRetentionHandle {
    /// Stop the background task and wait for the worker thread to finish.
    ///
    /// This consumes the handle. Dropping it has the same effect.
    pub fn stop(mut self) {
        self.shutdown.store(true, Ordering::SeqCst);
        if let Some(join) = self.join.take() {
            let _ = join.join();
        }
    }
}

impl Drop for LogRetentionHandle {
    fn drop(&mut self) {
        self.shutdown.store(true, Ordering::SeqCst);
        if let Some(join) = self.join.take() {
            let _ = join.join();
        }
    }
}

/// Start a background task that periodically enforces the log-file retention limit.
///
/// Unlike the one-shot cleanup at initialization, this keeps the log directory bounded for
/// processes that run across many daily rotations. The worker thread sleeps for `interval`
/// between runs (waking early if the handle is dropped/stopped) and deletes files older than
/// `max_log_files` each time.
///
/// - `env_prefix`: prefix used for the `LOG_DIR` / `LOG_PREFIX` / `LOG_MAX_FILES` lookups; see
///   [`crate::InitOptions::env_prefix`].
/// - `log_dir`: overrides the prefixed `LOG_DIR` when `Some`; otherwise [`DEFAULT_LOG_DIR`] is used.
/// - `log_prefix`: overrides the prefixed `LOG_PREFIX` when `Some`; otherwise [`DEFAULT_LOG_PREFIX`]
///   is used.
/// - `max_log_files`: overrides the prefixed `LOG_MAX_FILES` when `Some`; otherwise
///   [`DEFAULT_MAX_LOG_FILES`] is used. Values below `1` are treated as `1`, so the file currently
///   being written is never removed.
/// - `interval`: how often cleanup runs. Use [`DEFAULT_RETENTION_INTERVAL`] for the default.
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
    // Unlike the startup path, the file for the current day already exists here, so keep at
    // least one file to avoid deleting the active log file.
    let max_files = max_log_files
        .unwrap_or_else(|| resolve_max_log_files(env_prefix))
        .max(1);

    let shutdown = std::sync::Arc::new(AtomicBool::new(false));
    let shutdown_clone = shutdown.clone();

    let join = std::thread::Builder::new()
        .name("ngy-log-retention".to_string())
        .spawn(move || {
            // Step granularity keeps shutdown responsive without busy-waiting.
            let step = std::time::Duration::from_secs(1);
            // Clamp the interval to at least one step: a zero interval would otherwise turn
            // the loop into a CPU-burning busy loop.
            let interval = interval.max(step);
            while !shutdown_clone.load(Ordering::SeqCst) {
                if let Err(err) = cleanup_old_logs(&log_dir, &log_prefix, max_files) {
                    eprintln!("[WARN] log retention cleanup failed: {err}");
                }
                let mut slept = std::time::Duration::ZERO;
                while slept < interval && !shutdown_clone.load(Ordering::SeqCst) {
                    std::thread::sleep(step);
                    slept += step;
                }
            }
        })?;

    Ok(LogRetentionHandle {
        shutdown,
        join: Some(join),
    })
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

    // All tests read the RUST_LOG / LOG_DIR / LOG_PREFIX environment variables; they hold
    // crate::TEST_ENV_MUTEX uniformly, serialized with the tests in the console_tracing module,
    // to avoid concurrent read/write (unsafe) causing UB.
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

    #[test]
    fn resolve_retention_interval_defaults_to_none() {
        let _lock = lock_env();
        let name = env_name(RETENTION_INTERVAL_ENV);
        // SAFETY: unsafe in Rust 2024; runtime tests hold TEST_ENV_MUTEX for all env access.
        unsafe { std::env::remove_var(&name) };
        assert_eq!(resolve_retention_interval(TEST_PREFIX), None);
    }

    #[test]
    fn resolve_retention_interval_parses_seconds() {
        let _lock = lock_env();
        let name = env_name(RETENTION_INTERVAL_ENV);
        // SAFETY: unsafe in Rust 2024; runtime tests hold TEST_ENV_MUTEX for all env access.
        unsafe { std::env::set_var(&name, "3600") };
        assert_eq!(
            resolve_retention_interval(TEST_PREFIX),
            Some(std::time::Duration::from_secs(3600))
        );

        // Zero (and invalid) values are treated as disabled.
        // SAFETY: Same as above.
        unsafe { std::env::set_var(&name, "0") };
        assert_eq!(resolve_retention_interval(TEST_PREFIX), None);

        // SAFETY: Same as above; restore the environment this test changed.
        unsafe { std::env::remove_var(&name) };
    }

    #[test]
    fn resolve_retention_interval_or_default_is_enabled_by_default() {
        let _lock = lock_env();
        let name = env_name(RETENTION_INTERVAL_ENV);

        // Unset -> default interval (periodic retention on by default).
        // SAFETY: unsafe in Rust 2024; runtime tests hold TEST_ENV_MUTEX for all env access.
        unsafe { std::env::remove_var(&name) };
        assert_eq!(
            resolve_retention_interval_or_default(TEST_PREFIX),
            Some(DEFAULT_RETENTION_INTERVAL)
        );

        // Invalid -> default interval as well.
        // SAFETY: Same as above.
        unsafe { std::env::set_var(&name, "not-a-number") };
        assert_eq!(
            resolve_retention_interval_or_default(TEST_PREFIX),
            Some(DEFAULT_RETENTION_INTERVAL)
        );

        // Positive value -> used as-is.
        // SAFETY: Same as above.
        unsafe { std::env::set_var(&name, "120") };
        assert_eq!(
            resolve_retention_interval_or_default(TEST_PREFIX),
            Some(std::time::Duration::from_secs(120))
        );

        // Explicit zero -> disabled.
        // SAFETY: Same as above.
        unsafe { std::env::set_var(&name, "0") };
        assert_eq!(resolve_retention_interval_or_default(TEST_PREFIX), None);

        // SAFETY: Same as above; restore the environment this test changed.
        unsafe { std::env::remove_var(&name) };
    }

    #[test]
    fn cleanup_old_logs_keeps_most_recent_by_name_date() {
        let _lock = lock_env();
        let dir =
            std::env::temp_dir().join(format!("ngy_utils_tracing_test_{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();

        let prefix = "app.log";
        // Create the newest name first, so modification times are the *opposite* of the dates
        // embedded in the names. Ordering by mtime would keep the wrong set of files; ordering
        // by name date must win.
        for day in (1..=10).rev() {
            std::thread::sleep(std::time::Duration::from_millis(15));
            let path = dir.join(format!("{prefix}.2026-09-{day:02}"));
            std::fs::write(&path, b"x").unwrap();
        }

        cleanup_old_logs(dir.to_str().unwrap(), prefix, 7).unwrap();

        let remaining: Vec<String> = std::fs::read_dir(&dir)
            .unwrap()
            .filter_map(|e| e.ok())
            .map(|e| e.file_name().to_string_lossy().to_string())
            .filter(|name| name.starts_with(&format!("{prefix}.")))
            .collect();
        // Only the 7 most recent *dates* should remain, regardless of modification time.
        assert_eq!(remaining.len(), 7, "remaining: {remaining:?}");
        for day in 4..=10 {
            let name = format!("{prefix}.2026-09-{day:02}");
            assert!(remaining.contains(&name), "expected {} to be kept", name);
        }
        for day in 1..=3 {
            let name = format!("{prefix}.2026-09-{day:02}");
            assert!(
                !remaining.contains(&name),
                "expected {} to be removed",
                name
            );
        }

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn cleanup_old_logs_ignores_non_log_files() {
        let _lock = lock_env();
        let dir = std::env::temp_dir().join(format!(
            "ngy_utils_tracing_unrelated_{}",
            std::process::id()
        ));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();

        let prefix = "app.log";
        let real_log = dir.join(format!("{prefix}.2026-09-01"));
        std::fs::write(&real_log, b"x").unwrap();
        // These share the prefix but are not rotated logs and must be preserved.
        let logger = dir.join("app.logger");
        let backup = dir.join("app.log.backup");
        std::fs::write(&logger, b"x").unwrap();
        std::fs::write(&backup, b"x").unwrap();

        // Keep nothing: only the real log file may be removed.
        cleanup_old_logs(dir.to_str().unwrap(), prefix, 0).unwrap();

        assert!(!real_log.exists(), "rotated log should be removed");
        assert!(logger.exists(), "app.logger must be preserved");
        assert!(backup.exists(), "app.log.backup must be preserved");

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn cleanup_old_logs_leaves_room_for_the_new_file() {
        let _lock = lock_env();
        let dir =
            std::env::temp_dir().join(format!("ngy_utils_tracing_room_{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();

        let prefix = "app.log";
        let max_files = 7usize;
        for day in 1..=max_files {
            std::fs::write(dir.join(format!("{prefix}.2026-09-{day:02}")), b"x").unwrap();
        }

        // `build_file_layer` trims to `max_files - 1` before the current day's file is created,
        // so the directory ends up with exactly `max_files` files instead of `max_files + 1`.
        cleanup_old_logs(dir.to_str().unwrap(), prefix, max_files - 1).unwrap();

        let remaining = std::fs::read_dir(&dir).unwrap().count();
        assert_eq!(remaining, max_files - 1, "one slot must be reserved");

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn retention_task_keeps_recent_files_then_stops() {
        let _lock = lock_env();
        let dir = std::env::temp_dir().join(format!(
            "ngy_utils_tracing_retention_{}",
            std::process::id()
        ));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();

        let prefix = "app.log";
        for i in 0..10 {
            std::thread::sleep(std::time::Duration::from_millis(15));
            let path = dir.join(format!("{}.2026-09-{:02}", prefix, i + 1));
            std::fs::write(&path, b"x").unwrap();
        }

        // Run the background task with a very short interval so it triggers at least once.
        let handle = start_log_retention(
            TEST_PREFIX,
            Some(dir.to_string_lossy().to_string()),
            Some(prefix.to_string()),
            Some(7),
            std::time::Duration::from_millis(20),
        )
        .unwrap();

        // Give the task time to run cleanup at least once.
        std::thread::sleep(std::time::Duration::from_millis(200));

        let remaining: Vec<String> = std::fs::read_dir(&dir)
            .unwrap()
            .filter_map(|e| e.ok())
            .map(|e| e.file_name().to_string_lossy().to_string())
            .filter(|name| name.starts_with(prefix))
            .collect();
        assert!(
            remaining.len() <= 7,
            "retention task should bound files to <= 7, got {}",
            remaining.len()
        );

        // Dropping the handle must stop the thread without panicking.
        drop(handle);
        let _ = std::fs::remove_dir_all(&dir);
    }
}
