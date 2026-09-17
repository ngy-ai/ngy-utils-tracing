//! Log-retention integration tests.
//!
//! The retention policy is applied by the file writer whenever it rotates to a new day (see
//! `src/rolling_file.rs`), which is covered by that module's unit tests. These tests exercise the
//! public `cleanup_old_logs` contract from a consumer's point of view instead: which files count as
//! logs, which ones are kept and how they are ordered.

use ngy_utils_tracing::cleanup_old_logs;

/// An empty temporary directory, named after the test and this process.
fn temp_dir(label: &str) -> std::path::PathBuf {
    let dir =
        std::env::temp_dir().join(format!("ngy_utils_tracing_{label}_{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

#[test]
fn cleanup_old_logs_keeps_most_recent_by_name_date() {
    let dir = temp_dir("test");
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
    let dir = temp_dir("unrelated");
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
