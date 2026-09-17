//! The file writer behind the file layer: one file per day, named after the resolved offset's date.
//!
//! `tracing_appender::rolling` names its files from the **UTC** clock and cannot be told otherwise —
//! `Rotation` samples `OffsetDateTime::now_utc()` and keeps its date format private — so with a
//! non-UTC [`InitOptions::time_offset`](crate::InitOptions::time_offset) the file name would
//! contradict the timestamps inside it. This module supplies the small writer that keeps the two in
//! agreement: it appends to `{prefix}.{YYYY-MM-DD}` for the day the resolved offset is on, and
//! switches to the next file when that day changes.
//!
//! Retention is applied at rotation time — the moment the set of files changes — through
//! [`cleanup_old_logs`], so keeping a directory bounded needs neither a background task nor an
//! interval setting: a long-running process trims it whenever it rolls over.

use crate::retention::cleanup_old_logs;
use std::io::{self, Write};
use std::path::Path;
use time::{OffsetDateTime, UtcOffset};

/// The file currently being appended to, together with the day it holds.
struct OpenFile {
    /// Day the file was opened for, in the writer's offset; a write on another day rotates.
    day: time::Date,
    /// Append handle, created on the first write of its day.
    file: std::fs::File,
}

/// Daily-rotating file writer whose file names follow the resolved timestamp offset.
///
/// Writes go to `{log_dir}/{log_prefix}.{YYYY-MM-DD}`, where the date is the day `now` falls on in
/// `time_offset` — the same offset the layers print their timestamps with, so a record and the name
/// of the file holding it always agree.
///
/// Nothing is created until the first record arrives; that write opens the file for the current day
/// and trims the directory to `max_files` ([`cleanup_old_logs`]), and every later rotation does the
/// same. A `max_files` below `1` is treated as `1`, so the file being written is never removed.
///
/// The writer is wrapped in [`tracing_appender::non_blocking`](tracing_appender::non_blocking) by
/// [`crate::file_tracing`], so rotating happens on that worker thread rather than on the thread that
/// emitted the record.
pub(crate) struct RollingFileWriter {
    /// Log directory, which the caller has created.
    log_dir: String,
    /// File name prefix; the date is appended to it.
    log_prefix: String,
    /// Number of files to keep, including the one being written; at least `1`.
    max_files: usize,
    /// Offset deciding both the file name date and the timestamps inside the file.
    time_offset: UtcOffset,
    /// File of the current day, or `None` until the first write.
    open: Option<OpenFile>,
}

impl RollingFileWriter {
    /// Create a writer for `log_dir`, which must already exist.
    pub(crate) fn new(
        log_dir: &str,
        log_prefix: &str,
        max_files: usize,
        time_offset: UtcOffset,
    ) -> Self {
        Self {
            log_dir: log_dir.to_string(),
            log_prefix: log_prefix.to_string(),
            // Keeping at least the file that is being appended to is not negotiable: deleting it
            // would lose the records already buffered for it.
            max_files: max_files.max(1),
            time_offset,
            open: None,
        }
    }

    /// [`Write::write`] with an explicit clock, which also makes rotation testable without waiting
    /// for a day to pass.
    fn write_at(&mut self, now: OffsetDateTime, buf: &[u8]) -> io::Result<usize> {
        self.open_for(now)?.write(buf)
    }

    /// The append handle for the day `now` falls on, rotating first when the day changed.
    fn open_for(&mut self, now: OffsetDateTime) -> io::Result<&mut std::fs::File> {
        let day = now.to_offset(self.time_offset).date();
        if self.open.as_ref().is_none_or(|open| open.day != day) {
            self.rotate(day)?;
        }

        match self.open.as_mut() {
            Some(open) => Ok(&mut open.file),
            // Only reachable if `rotate` returned `Ok` without storing a file, which it never does.
            None => Err(io::Error::other("the log file was not opened")),
        }
    }

    /// Open the file of `day` for appending and enforce the retention limit.
    fn rotate(&mut self, day: time::Date) -> io::Result<()> {
        let name = format!("{}.{}", self.log_prefix, day);
        let path = Path::new(&self.log_dir).join(name);
        let file = std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(&path)?;
        self.open = Some(OpenFile { day, file });

        // Rotation is the moment the set of files changes, so it is also the moment to trim it. The
        // new file exists by now, which is why the limit is `max_files` and not one less.
        if let Err(err) = cleanup_old_logs(&self.log_dir, &self.log_prefix, self.max_files) {
            crate::warn(format_args!(
                "log retention after rotating to {} failed: {}",
                path.display(),
                err
            ));
        }

        Ok(())
    }
}

impl Write for RollingFileWriter {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        self.write_at(OffsetDateTime::now_utc(), buf)
    }

    fn flush(&mut self) -> io::Result<()> {
        match self.open.as_mut() {
            // Nothing has been written yet, so there is no file to flush.
            None => Ok(()),
            Some(open) => open.file.flush(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use time::macros::datetime;

    // The writer is crate-private and its rotation is driven by a clock, so these tests live here:
    // passing a fixed `now` is the only way to exercise "the day changed" without waiting for
    // midnight. `cleanup_old_logs` itself is covered through the public API in
    // `tests/retention_test.rs`.

    /// An empty temporary directory, named after the test and this process.
    fn temp_dir(label: &str) -> std::path::PathBuf {
        let dir = std::env::temp_dir().join(format!(
            "ngy_utils_tracing_writer_{label}_{}",
            std::process::id()
        ));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    /// Sorted names of the files in `dir`.
    fn file_names(dir: &std::path::Path) -> Vec<String> {
        let mut names: Vec<String> = std::fs::read_dir(dir)
            .unwrap()
            .filter_map(|entry| entry.ok())
            .map(|entry| entry.file_name().to_string_lossy().to_string())
            .collect();
        names.sort();
        names
    }

    fn offset(hours: i8, minutes: i8) -> UtcOffset {
        UtcOffset::from_hms(hours, minutes, 0).unwrap()
    }

    /// The date in the file name is the day `now` falls on **in the configured offset**, so it
    /// agrees with the timestamps the layers write into that file.
    #[test]
    fn file_names_follow_the_configured_offset() {
        let dir = temp_dir("offset");
        // 18:30 UTC has already rolled over in `+08:00`, but not in `-05:00`.
        let now = datetime!(2026-09-17 18:30 UTC);

        let mut eastern = RollingFileWriter::new(dir.to_str().unwrap(), "app.log", 7, offset(8, 0));
        eastern.write_at(now, b"x").unwrap();
        assert!(dir.join("app.log.2026-09-18").exists());

        let mut western =
            RollingFileWriter::new(dir.to_str().unwrap(), "app.log", 7, offset(-5, 0));
        western.write_at(now, b"x").unwrap();
        assert!(dir.join("app.log.2026-09-17").exists());

        let _ = std::fs::remove_dir_all(&dir);
    }

    /// Writes of one day share a file, the first write of a new day opens the next one, and the
    /// directory is trimmed to `max_files` at that moment.
    #[test]
    fn rotation_opens_the_next_day_and_trims_the_directory() {
        let dir = temp_dir("rotate");
        let mut writer =
            RollingFileWriter::new(dir.to_str().unwrap(), "app.log", 2, UtcOffset::UTC);
        let noon = |day: u8| datetime!(2026-09-01 12:00 UTC).replace_day(day).unwrap();

        for day in 1..=4 {
            writer.write_at(noon(day), b"x").unwrap();
            // The second write of the same day must not open another file.
            writer.write_at(noon(day), b"y").unwrap();
        }

        // `max_files = 2` keeps the current day and the one before it.
        assert_eq!(
            file_names(&dir),
            ["app.log.2026-09-03", "app.log.2026-09-04"]
        );
        assert_eq!(
            std::fs::read(dir.join("app.log.2026-09-04")).unwrap(),
            b"xy"
        );

        let _ = std::fs::remove_dir_all(&dir);
    }

    /// A limit below one is treated as one: retention must never delete the file being written.
    #[test]
    fn a_limit_below_one_still_keeps_the_file_being_written() {
        let dir = temp_dir("minimum");
        let mut writer =
            RollingFileWriter::new(dir.to_str().unwrap(), "app.log", 0, UtcOffset::UTC);

        writer
            .write_at(datetime!(2026-09-01 12:00 UTC), b"x")
            .unwrap();
        writer
            .write_at(datetime!(2026-09-02 12:00 UTC), b"y")
            .unwrap();

        assert_eq!(file_names(&dir), ["app.log.2026-09-02"]);
        assert_eq!(std::fs::read(dir.join("app.log.2026-09-02")).unwrap(), b"y");

        let _ = std::fs::remove_dir_all(&dir);
    }
}
