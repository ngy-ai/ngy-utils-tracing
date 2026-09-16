//! File logging initialization: writes JSON-format logs to a specified directory.
//!
//! Log files rotate daily, with filename format `{prefix}.{date}`.
//! The log directory is controlled via the `LOG_DIR` environment variable (default `logs`).
//! The log file prefix is controlled via the `LOG_PREFIX` environment variable (default `app.log`).
//! The number of log files retained is controlled via the `LOG_MAX_FILES` environment variable
//! (default `7`); on initialization the oldest files beyond this limit are deleted, leaving room
//! for the file created for the current day so the directory never holds more than `LOG_MAX_FILES`.
//! The log level is controlled via the `RUST_LOG` environment variable (default `info`).

use std::sync::atomic::{AtomicBool, Ordering};
use tracing_appender::non_blocking::WorkerGuard;
use tracing_subscriber::{
    EnvFilter, Layer,
    fmt::{self},
    layer::SubscriberExt,
};

/// Default log directory
const DEFAULT_LOG_DIR: &str = "logs";

/// Default log file prefix
const DEFAULT_LOG_PREFIX: &str = "app.log";

/// Default number of retained log files; older files beyond this are deleted on init.
const DEFAULT_MAX_LOG_FILES: usize = 7;

/// Default interval at which the background retention task runs [`cleanup_old_logs`].
pub const DEFAULT_RETENTION_INTERVAL: std::time::Duration = std::time::Duration::from_secs(3600);

/// Build the log directory path
pub fn resolve_log_dir() -> String {
    std::env::var("LOG_DIR").unwrap_or_else(|_| DEFAULT_LOG_DIR.to_string())
}

/// Build the log file prefix
pub fn resolve_log_prefix() -> String {
    std::env::var("LOG_PREFIX").unwrap_or_else(|_| DEFAULT_LOG_PREFIX.to_string())
}

/// Resolve the maximum number of retained log files.
///
/// Reads `LOG_MAX_FILES`; falls back to [`DEFAULT_MAX_LOG_FILES`] (7) when unset
/// or when the value is not a positive integer.
pub fn resolve_max_log_files() -> usize {
    std::env::var("LOG_MAX_FILES")
        .ok()
        .and_then(|v| v.parse::<usize>().ok())
        .filter(|&n| n > 0)
        .unwrap_or(DEFAULT_MAX_LOG_FILES)
}

/// Resolve the background retention interval from the `LOG_RETENTION_INTERVAL_SECONDS` environment variable.
///
/// The value is interpreted as **seconds**. Returns `None` when unset or when the value is `0`
/// (disabled), so callers fall back to running no background task unless an explicit interval is
/// provided via [`InitOptions::retention_interval`].
///
/// Note: [`crate::init`] does not use this function directly; it uses
/// [`resolve_retention_interval_or_default`] so that periodic retention is enabled by default.
pub fn resolve_retention_interval() -> Option<std::time::Duration> {
    std::env::var("LOG_RETENTION_INTERVAL_SECONDS")
        .ok()
        .and_then(|v| v.parse::<u64>().ok())
        .filter(|&secs| secs > 0)
        .map(std::time::Duration::from_secs)
}

/// Resolve the effective background retention interval used by [`crate::init`].
///
/// Unlike [`resolve_retention_interval`], this keeps periodic retention **enabled by default**:
///
/// - `LOG_RETENTION_INTERVAL_SECONDS=0` → `None` (the background task is explicitly disabled)
/// - a positive value → that many seconds
/// - unset or invalid → [`DEFAULT_RETENTION_INTERVAL`] (currently 1 hour)
pub(crate) fn resolve_retention_interval_or_default() -> Option<std::time::Duration> {
    match std::env::var("LOG_RETENTION_INTERVAL_SECONDS") {
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
/// - `log_dir`: overrides `LOG_DIR` when `Some`; otherwise `LOG_DIR` (default `logs`) is used.
/// - `log_prefix`: overrides `LOG_PREFIX` when `Some`; otherwise `LOG_PREFIX` (default `app.log`).
/// - `max_log_files`: overrides `LOG_MAX_FILES` when `Some`; otherwise the default `7` is used.
///   Values below `1` are treated as `1`, so the file currently being written is never removed.
/// - `interval`: how often cleanup runs. Use [`DEFAULT_RETENTION_INTERVAL`] for the default (1h).
///
/// # Example
///
/// ```no_run
/// use ngy_utils_tracing::{start_log_retention, DEFAULT_RETENTION_INTERVAL};
///
/// // Keep the handle alive (like the WorkerGuard) for the process lifetime.
/// let _retention = start_log_retention(None, None, None, DEFAULT_RETENTION_INTERVAL).unwrap();
/// ```
///
/// # Errors
///
/// Returns an error if the worker thread cannot be spawned.
pub fn start_log_retention(
    log_dir: Option<String>,
    log_prefix: Option<String>,
    max_log_files: Option<usize>,
    interval: std::time::Duration,
) -> anyhow::Result<LogRetentionHandle> {
    let log_dir = log_dir.unwrap_or_else(resolve_log_dir);
    let log_prefix = log_prefix.unwrap_or_else(resolve_log_prefix);
    // Unlike the startup path, the file for the current day already exists here, so keep at
    // least one file to avoid deleting the active log file.
    let max_files = max_log_files.unwrap_or_else(resolve_max_log_files).max(1);

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

/// Build the EnvFilter, reading `RUST_LOG` first, otherwise using `info`
pub fn build_file_filter(default_directive: &str) -> anyhow::Result<EnvFilter> {
    let filter =
        EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new(default_directive));
    Ok(filter)
}

/// Build the file log layer (JSON format)
pub fn build_file_layer<S>(
    log_dir: &str,
    log_prefix: &str,
    max_files: usize,
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
        .with_target(true)
        .with_level(true)
        .with_ansi(false)
        .with_timer(fmt::time::SystemTime)
        .with_writer(non_blocking);

    Ok((layer, guard))
}

/// Initialize file logging: JSON format output to the log directory.
///
/// The returned `tracing_appender::non_blocking::WorkerGuard` must stay alive,
/// otherwise logs may be lost. The caller should keep it in the main function until the process exits.
///
/// - `log_dir`: overrides the `LOG_DIR` environment variable when `Some`; otherwise
///   `LOG_DIR` (default `logs`) is used.
/// - `log_prefix`: overrides the `LOG_PREFIX` environment variable when `Some`; otherwise
///   `LOG_PREFIX` (default `app.log`) is used as the file name prefix.
/// - `max_log_files`: overrides the `LOG_MAX_FILES` environment variable when `Some`;
///   otherwise `LOG_MAX_FILES` (default `7`) is used as the retention limit.
pub fn file_tracing(
    log_dir: Option<String>,
    log_prefix: Option<String>,
    max_log_files: Option<usize>,
) -> anyhow::Result<WorkerGuard> {
    let log_dir = log_dir.unwrap_or_else(resolve_log_dir);
    let log_prefix = log_prefix.unwrap_or_else(resolve_log_prefix);
    let max_files = max_log_files.unwrap_or_else(resolve_max_log_files);
    let (fmt_layer, guard) = build_file_layer(&log_dir, &log_prefix, max_files)?;
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

    #[test]
    fn resolve_max_log_files_defaults_to_seven() {
        let _lock = lock_env();
        // Unsafe in Rust 2024: safe here because TEST_ENV_MUTEX serializes all env access.
        unsafe { std::env::remove_var("LOG_MAX_FILES") };
        assert_eq!(resolve_max_log_files(), 7);
    }

    #[test]
    fn resolve_retention_interval_defaults_to_none() {
        let _lock = lock_env();
        // Unsafe in Rust 2024: safe here because TEST_ENV_MUTEX serializes all env access.
        unsafe { std::env::remove_var("LOG_RETENTION_INTERVAL_SECONDS") };
        assert_eq!(resolve_retention_interval(), None);
    }

    #[test]
    fn resolve_retention_interval_parses_seconds() {
        let _lock = lock_env();
        // Unsafe in Rust 2024: safe here because TEST_ENV_MUTEX serializes all env access.
        unsafe { std::env::set_var("LOG_RETENTION_INTERVAL_SECONDS", "3600") };
        assert_eq!(
            resolve_retention_interval(),
            Some(std::time::Duration::from_secs(3600))
        );
        // Zero (and invalid) values are treated as disabled.
        unsafe { std::env::set_var("LOG_RETENTION_INTERVAL_SECONDS", "0") };
        assert_eq!(resolve_retention_interval(), None);
        unsafe { std::env::remove_var("LOG_RETENTION_INTERVAL_SECONDS") };
    }

    #[test]
    fn resolve_retention_interval_or_default_is_enabled_by_default() {
        let _lock = lock_env();
        // Unsafe in Rust 2024: safe here because TEST_ENV_MUTEX serializes all env access.

        // Unset -> default interval (periodic retention on by default).
        unsafe { std::env::remove_var("LOG_RETENTION_INTERVAL_SECONDS") };
        assert_eq!(
            resolve_retention_interval_or_default(),
            Some(DEFAULT_RETENTION_INTERVAL)
        );

        // Invalid -> default interval as well.
        unsafe { std::env::set_var("LOG_RETENTION_INTERVAL_SECONDS", "not-a-number") };
        assert_eq!(
            resolve_retention_interval_or_default(),
            Some(DEFAULT_RETENTION_INTERVAL)
        );

        // Positive value -> used as-is.
        unsafe { std::env::set_var("LOG_RETENTION_INTERVAL_SECONDS", "120") };
        assert_eq!(
            resolve_retention_interval_or_default(),
            Some(std::time::Duration::from_secs(120))
        );

        // Explicit zero -> disabled.
        unsafe { std::env::set_var("LOG_RETENTION_INTERVAL_SECONDS", "0") };
        assert_eq!(resolve_retention_interval_or_default(), None);

        unsafe { std::env::remove_var("LOG_RETENTION_INTERVAL_SECONDS") };
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
