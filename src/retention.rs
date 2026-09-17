//! Log retention: removing old rotated log files, on demand and periodically.
//!
//! This module owns the retention policy — which files count as rotated logs, how they are ordered
//! and which of them are deleted — plus its own setting (the interval) and the background task that
//! keeps enforcing the policy while a long-running process rotates its file every day.
//!
//! [`cleanup_old_logs`] and [`spawn_retention_task`] work purely on their arguments: the settings
//! shared with the writer (log directory, name prefix, retention limit) are resolved by
//! [`crate::start_log_retention`] in `file_tracing` and passed in. Every setting is documented once
//! in [`InitOptions`](crate::InitOptions).

use std::sync::atomic::{AtomicBool, Ordering};

/// Default interval at which the background retention task runs [`cleanup_old_logs`].
pub const DEFAULT_RETENTION_INTERVAL: std::time::Duration = std::time::Duration::from_secs(3600);

/// Suffix of the environment variable holding the background retention interval, in seconds.
const RETENTION_INTERVAL_ENV: &str = "LOG_RETENTION_INTERVAL_SECONDS";

/// Resolve the background retention interval from the prefixed `LOG_RETENTION_INTERVAL_SECONDS`
/// variable.
///
/// The value is interpreted as **seconds**. Returns `None` when unset or when the value is `0`
/// (disabled), so callers fall back to running no background task unless an explicit interval is
/// provided via [`crate::InitOptions::retention_interval`].
///
/// Note: [`crate::init`] does not use this function directly; it uses
/// `resolve_retention_interval_or_default` so that periodic retention is enabled by default.
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

/// A candidate log file found in the log directory.
#[derive(Debug)]
struct LogFileEntry {
    /// Full path of the file.
    path: std::path::PathBuf,
    /// Date parsed from the file name, as `(year, month, day)`; `None` for a bare `{prefix}` file
    /// (or a name without a parseable date), which is treated as the oldest.
    date: Option<(i32, u8, u8)>,
    /// Modification time; only used to order files whose dates are equal (or missing).
    modified: std::time::SystemTime,
}

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
            Some(LogFileEntry {
                path,
                date,
                modified,
            })
        })
        .collect();

    // Oldest first so we can drop the front of the list. A name without a parseable date sorts as
    // the oldest (so undated leftovers are removed before dated files); the modification time only
    // breaks ties between files whose dates are equal or missing.
    files.sort_by(|a, b| match (a.date, b.date) {
        (Some(a_date), Some(b_date)) => a_date
            .cmp(&b_date)
            .then_with(|| a.modified.cmp(&b.modified)),
        (Some(_), None) => std::cmp::Ordering::Greater,
        (None, Some(_)) => std::cmp::Ordering::Less,
        (None, None) => a.modified.cmp(&b.modified),
    });

    let excess = files.len().saturating_sub(max_files);
    for entry in files.into_iter().take(excess) {
        // Deleting a file that is currently open for writing would be a user error
        // (the active file carries the newest date and is kept); ignore failures.
        if let Err(err) = std::fs::remove_file(&entry.path) {
            eprintln!(
                "[WARN] failed to remove old log file {}: {}",
                entry.path.display(),
                err
            );
        }
    }

    Ok(())
}

/// Handle for a background log-retention task started by [`crate::start_log_retention`].
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

/// Spawn the background retention task for settings that are already resolved.
///
/// This is the engine behind [`crate::start_log_retention`], which resolves the settings first. The
/// worker thread calls [`cleanup_old_logs`] every `interval`, sleeping in one-second steps so a
/// stopped or dropped handle is noticed promptly.
///
/// `max_files` below `1` is treated as `1`: once a long-running process enforces retention, the
/// file for the current day already exists, so keeping at least one file avoids deleting the file
/// that is being written.
///
/// # Errors
///
/// Returns an error if the worker thread cannot be spawned.
pub(crate) fn spawn_retention_task(
    log_dir: String,
    log_prefix: String,
    max_files: usize,
    interval: std::time::Duration,
) -> anyhow::Result<LogRetentionHandle> {
    let max_files = max_files.max(1);

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

#[cfg(test)]
mod tests {
    use super::*;

    // Only the test below reads environment variables; it holds crate::TEST_ENV_MUTEX, serialized
    // with the other env-accessing tests in the lib test binary, to avoid concurrent read/write
    // (unsafe) causing UB. The rest of the retention behaviour is covered by
    // `tests/retention_test.rs` through the public API.
    fn lock_env() -> std::sync::MutexGuard<'static, ()> {
        crate::TEST_ENV_MUTEX.lock().unwrap()
    }

    /// Prefix used by the tests so they never touch a variable name a real deployment could use.
    const TEST_PREFIX: &str = "NGY_TEST_";

    #[test]
    fn resolve_retention_interval_or_default_is_enabled_by_default() {
        let _lock = lock_env();
        let name = crate::env_var_name(TEST_PREFIX, RETENTION_INTERVAL_ENV);

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
}
