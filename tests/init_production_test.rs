//! Production mode initialization integration test
//!
//! Verify that init() uses file logging (JSON) in Production mode, returns a guard, and writes into
//! a file whose name repeats the date of the records inside it — both follow the resolved timestamp
//! offset, so a UTC-named file would be caught here.
//! This file exists separately because the tracing subscriber can only be initialized once.

use ngy_utils_tracing::{AppMode, InitOptions, get_current_mode, init};

/// Prefix for the environment variables the crate may read in this test.
const ENV_PREFIX: &str = "NGY_TEST_";

/// Newest `logs/app.log.*` file and its content.
fn newest_log_file() -> (std::path::PathBuf, String) {
    let mut files: Vec<(std::time::SystemTime, std::path::PathBuf)> = std::fs::read_dir("logs")
        .expect("the default log directory must exist after file-logging init")
        .filter_map(|entry| entry.ok())
        .filter(|entry| entry.file_name().to_string_lossy().starts_with("app.log."))
        .filter_map(|entry| {
            let modified = entry.metadata().ok()?.modified().ok()?;
            Some((modified, entry.path()))
        })
        .collect();
    files.sort_by_key(|(modified, _)| *modified);

    let path = files
        .pop()
        .expect("a log file must have been written by this test")
        .1;
    let content = std::fs::read_to_string(&path).expect("log files are written as UTF-8 JSON");
    (path, content)
}

#[test]
fn test_full_init_production() {
    // Specify production mode via mode_override to avoid modifying process env vars (unsafe)
    let result = init(InitOptions::new(ENV_PREFIX).mode_override(Some("production".to_string())))
        .expect("init should succeed");

    assert_eq!(result.mode, AppMode::Production);
    // Production mode uses file logging and should return a guard
    assert!(result.guard.is_some());
    // After init, the current mode can be obtained via get_current_mode
    assert_eq!(get_current_mode(), Some(AppMode::Production));

    // Verify tracing can be used (writes JSON file logs)
    tracing::info!("Test log message from production mode");

    // Dropping the guard flushes the non-blocking writer, so the record is on disk afterwards.
    drop(result.guard);

    // The file holding the record must be named after the date its timestamp shows: the name and the
    // timestamp both come from the resolved offset (the machine's time zone by default), so a writer
    // that dated its files from a different zone would land on a different day.
    let (path, record) = newest_log_file();
    let timestamp_date = record
        .split("\"timestamp\":\"")
        .nth(1)
        .and_then(|rest| rest.get(..10))
        .expect("the JSON record must carry an RFC 3339 timestamp");
    assert_eq!(
        path.file_name().unwrap().to_string_lossy(),
        format!("app.log.{timestamp_date}"),
        "the file name must repeat the date of the record it holds"
    );
}
