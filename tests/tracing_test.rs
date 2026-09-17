//! Logging system integration tests
//!
//! Tests the public API of the logging module from an external consumer's perspective.

use ngy_utils_tracing::{build_debug_filter, build_file_filter, resolve_log_dir};
use std::sync::Mutex;

/// Prefix for the environment lookups in this test; the crate never reads unprefixed names.
const ENV_PREFIX: &str = "NGY_TEST_";

// Serialize all tests in this binary that read/write the RUST_LOG env var to avoid race conditions
// (unsafe env access). build_file_filter / build_debug_filter read RUST_LOG internally, and two of
// the cases write/restore RUST_LOG, so all must be serialized; otherwise concurrent read/write of
// the process env var is UB.
static ENV_MUTEX: Mutex<()> = Mutex::new(());

fn lock_env() -> std::sync::MutexGuard<'static, ()> {
    ENV_MUTEX.lock().unwrap()
}

#[test]
fn resolve_log_dir_returns_non_empty_string() {
    let _lock = lock_env();
    let dir = resolve_log_dir(ENV_PREFIX);
    assert!(!dir.is_empty());
}

#[test]
fn build_file_filter_with_info() {
    let _lock = lock_env();
    let filter = build_file_filter("info").unwrap();
    let _ = format!("{:?}", filter);
}

#[test]
fn build_file_filter_with_debug() {
    let _lock = lock_env();
    let filter = build_file_filter("debug").unwrap();
    let _ = format!("{:?}", filter);
}

#[test]
fn build_file_filter_with_warn() {
    let _lock = lock_env();
    let filter = build_file_filter("warn").unwrap();
    let _ = format!("{:?}", filter);
}

#[test]
fn build_debug_filter_returns_valid_filter() {
    let _lock = lock_env();
    let filter = build_debug_filter(Vec::new());
    let _ = format!("{:?}", filter);
}

#[test]
fn multiple_filters_can_coexist() {
    let _lock = lock_env();
    let info = build_file_filter("info").unwrap();
    let debug = build_file_filter("debug").unwrap();
    let warn = build_file_filter("warn").unwrap();

    // Verify they can be constructed independently
    let _ = format!("{:?}{:?}{:?}", info, debug, warn);
}

#[test]
fn console_and_file_filters_can_coexist() {
    let _lock = lock_env();
    let project = build_debug_filter(Vec::new());
    let file = build_file_filter("info").unwrap();

    // Verify they can be constructed independently
    let _ = format!("{:?}{:?}", project, file);
}

#[test]
fn file_filter_respects_rust_log_env_var() {
    // SAFETY: From Rust 2024, modifying process env vars is unsafe. Holding the ENV_MUTEX lock ensures
    // the write/restore of RUST_LOG runs serialized with other tests reading RUST_LOG in the same
    // binary, avoiding race conditions.
    let _lock = lock_env();
    unsafe {
        std::env::set_var("RUST_LOG", "debug");
    }
    let filter = build_file_filter("info").unwrap();
    let _ = format!("{:?}", filter);
    // SAFETY: Same as above; restore the env var set by this test.
    unsafe {
        std::env::remove_var("RUST_LOG");
    }
}

#[test]
fn debug_filter_respects_rust_log_env_var() {
    // SAFETY: From Rust 2024, modifying process env vars is unsafe. Holding the ENV_MUTEX lock ensures
    // the write/restore of RUST_LOG runs serialized with other tests reading RUST_LOG in the same
    // binary, avoiding race conditions.
    let _lock = lock_env();
    unsafe {
        std::env::set_var("RUST_LOG", "warn");
    }
    let rendered = build_debug_filter(vec!["commons".to_string()]).to_string();
    // RUST_LOG is layered on top of the crate directives: it must not drop `commons=debug`.
    assert!(rendered.contains("commons=debug"), "rendered: {rendered}");
    assert!(rendered.contains("warn"), "rendered: {rendered}");
    // SAFETY: Same as above; restore the env var set by this test.
    unsafe {
        std::env::remove_var("RUST_LOG");
    }
}
