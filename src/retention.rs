//! Retention policy for rotated log files: which files count as rotated logs, how they are ordered
//! and which of them are deleted.
//!
//! [`cleanup_old_logs`] works purely on its arguments and is the single implementation of the
//! policy: the file writer calls it every time it rotates to a new day (see [`crate::rolling_file`]),
//! and callers that manage their own files can call it directly. The settings it needs (log
//! directory, name prefix, retention limit) are resolved in [`crate::file_tracing`] from
//! [`InitOptions`](crate::InitOptions), which documents them once.
//!
//! Rotated files are named `{prefix}.{YYYY-MM-DD}`, where the date is the day the writer's resolved
//! offset is on — the same offset as the timestamps inside the file (see
//! [`InitOptions::time_offset`](crate::InitOptions::time_offset)), so a record and the name of the
//! file holding it always agree.

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
/// Returns `None` when the name is not `{prefix}.{date}` or the trailing part does not parse as a
/// date, so that unrelated files (e.g. `app.logger`) are never treated as log files. The component
/// ranges are only sanity-checked: month `13` is rejected, an impossible day such as `2026-02-31` is
/// not — the writer this crate ships cannot produce one.
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
/// Only daily log files are matched: a file counts as one when its name is exactly `log_prefix` or
/// has the form `{log_prefix}.{YYYY-MM-DD}` (e.g. `app.log.2026-09-16`). Unrelated files that merely
/// share the prefix (e.g. `app.logger` or `app.log.backup`) are left untouched.
///
/// Files are ordered by the date embedded in their name, which stays correct even when a
/// file's modification time has been altered by a copy or restore. Names without a parseable
/// date sort as the oldest, and modification time breaks ties.
///
/// `max_files == 0` keeps nothing and deletes every matching file; callers must make sure this does
/// not target a file that is currently open for writing — the file writer, which calls this right
/// after opening the file for the new day, keeps at least the newest file for that reason.
///
/// # Errors
///
/// Returns an error if the log directory cannot be read. A file that cannot be inspected or
/// deleted is reported and skipped instead: retention is best effort and must not fail the caller.
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
            let modified = match std::fs::metadata(&path).and_then(|m| m.modified()) {
                Ok(modified) => modified,
                Err(err) => {
                    // Without a modification time the file cannot be ordered against the others,
                    // so it is left alone rather than guessed at (or counted against `max_files`).
                    crate::warn(format_args!(
                        "cannot inspect candidate log file {}: {}",
                        path.display(),
                        err
                    ));
                    return None;
                }
            };
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
        // (the active file carries the newest date and is kept); report and continue.
        if let Err(err) = std::fs::remove_file(&entry.path) {
            crate::warn(format_args!(
                "failed to remove old log file {}: {}",
                entry.path.display(),
                err
            ));
        }
    }

    Ok(())
}
