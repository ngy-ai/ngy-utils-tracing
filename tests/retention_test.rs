//! Log-retention integration tests.
//!
//! These exercise the retention API the way a consumer sees it — `cleanup_old_logs`,
//! `start_log_retention` and `resolve_retention_interval` — including that only the prefixed
//! environment variables are read.

use ngy_utils_tracing::{cleanup_old_logs, resolve_retention_interval, start_log_retention};
use std::sync::Mutex;

/// Prefix used by the tests so they never touch a variable name a real deployment could use.
const ENV_PREFIX: &str = "NGY_TEST_";

// Serializes the tests in this binary that touch environment variables; the integration test binary
// is separate from the lib tests, so it needs its own mutex.
static ENV_MUTEX: Mutex<()> = Mutex::new(());

fn lock_env() -> std::sync::MutexGuard<'static, ()> {
    ENV_MUTEX.lock().unwrap()
}

/// Full name of the prefixed retention-interval variable, spelled out on purpose: it is the name
/// users are told to set, so the test must not rebuild it from an internal constant.
fn retention_interval_var() -> String {
    format!("{ENV_PREFIX}LOG_RETENTION_INTERVAL_SECONDS")
}

/// An empty temporary directory, named after the test and this process.
fn temp_dir(label: &str) -> std::path::PathBuf {
    let dir =
        std::env::temp_dir().join(format!("ngy_utils_tracing_{label}_{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

#[test]
fn resolve_retention_interval_ignores_the_unprefixed_variable() {
    let _lock = lock_env();
    let name = retention_interval_var();
    // SAFETY: unsafe in Rust 2024; this file serializes its env access with ENV_MUTEX.
    unsafe {
        std::env::remove_var(&name);
        // A decoy: another program owning the unprefixed name must not influence us.
        std::env::set_var("LOG_RETENTION_INTERVAL_SECONDS", "30");
    }
    assert_eq!(resolve_retention_interval(ENV_PREFIX), None);

    // SAFETY: Same as above.
    unsafe { std::env::set_var(&name, "3600") };
    assert_eq!(
        resolve_retention_interval(ENV_PREFIX),
        Some(std::time::Duration::from_secs(3600))
    );

    // Zero (and invalid) values are treated as disabled.
    // SAFETY: Same as above.
    unsafe { std::env::set_var(&name, "0") };
    assert_eq!(resolve_retention_interval(ENV_PREFIX), None);

    // SAFETY: Same as above; restore the environment this test changed.
    unsafe {
        std::env::remove_var(&name);
        std::env::remove_var("LOG_RETENTION_INTERVAL_SECONDS");
    }
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

#[test]
fn cleanup_old_logs_leaves_room_for_the_new_file() {
    let dir = temp_dir("room");
    let prefix = "app.log";
    let max_files = 7usize;
    for day in 1..=max_files {
        std::fs::write(dir.join(format!("{prefix}.2026-09-{day:02}")), b"x").unwrap();
    }

    // The startup path trims to `max_files - 1` before the current day's file is created, so the
    // directory ends up with exactly `max_files` files instead of `max_files + 1`.
    cleanup_old_logs(dir.to_str().unwrap(), prefix, max_files - 1).unwrap();

    let remaining = std::fs::read_dir(&dir).unwrap().count();
    assert_eq!(remaining, max_files - 1, "one slot must be reserved");

    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn retention_task_keeps_recent_files_then_stops() {
    let dir = temp_dir("retention");
    let prefix = "app.log";
    for i in 0..10 {
        std::thread::sleep(std::time::Duration::from_millis(15));
        let path = dir.join(format!("{}.2026-09-{:02}", prefix, i + 1));
        std::fs::write(&path, b"x").unwrap();
    }

    // Run the background task with a very short interval so it triggers at least once.
    let handle = start_log_retention(
        ENV_PREFIX,
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
