//! Full initialization integration test
//!
//! Tests the complete init() call flow, including that only the prefixed environment variables are
//! read — an unrelated `APP_MODE` belonging to another program must be ignored.
//! This file exists separately because the tracing subscriber can only be initialized once.

use ngy_utils_tracing::{AppMode, InitOptions, get_current_mode, init};
use std::env;

/// Prefix for the variables this test sets; the crate never reads unprefixed names.
const ENV_PREFIX: &str = "NGY_TEST_";

#[test]
fn test_full_init_development() {
    // SAFETY: This test file is the only test that modifies process env vars (the tracing subscriber
    // can only be initialized once, so related cases are isolated in this file); there are no other
    // tests concurrently accessing env vars.
    unsafe {
        env::set_var(format!("{ENV_PREFIX}APP_MODE"), "development");
        // Decoy: another program using the unprefixed name must not be picked up by this crate.
        env::set_var("APP_MODE", "production");
        env::remove_var("RUST_LOG");
    }

    let result = init(InitOptions::new(ENV_PREFIX)).expect("init should succeed");

    assert_eq!(result.mode, AppMode::Development);
    assert!(result.guard.is_none());
    // After init, the current mode can be obtained via get_current_mode
    assert_eq!(get_current_mode(), Some(AppMode::Development));

    // Verify tracing can be used
    tracing::info!("Test log message from development mode");

    // SAFETY: Same as above; this test exclusively modifies process env vars, no concurrent access.
    unsafe {
        env::remove_var(format!("{ENV_PREFIX}APP_MODE"));
        env::remove_var("APP_MODE");
    }
}
