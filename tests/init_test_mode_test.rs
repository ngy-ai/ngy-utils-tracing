//! Test mode initialization integration test
//!
//! Verify that init() outputs to both console and file in Test mode and returns a guard.
//! This file exists separately because the tracing subscriber can only be initialized once.

use ngy_utils_tracing::{AppMode, InitOptions, get_current_mode, init};

/// Prefix for the environment variables the crate may read in this test.
const ENV_PREFIX: &str = "NGY_TEST_";

#[test]
fn test_full_init_test_mode() {
    // Specify test mode via mode_override to avoid modifying process env vars (unsafe)
    let result = init(InitOptions::new(ENV_PREFIX).mode_override(Some("test".to_string())))
        .expect("init should succeed");

    assert_eq!(result.mode, AppMode::Test);
    // Test mode writes files simultaneously and should return a guard
    assert!(result.guard.is_some());
    // Periodic retention is enabled by default (1 hour unless overridden/disabled via env)
    assert!(result.retention.is_some());
    // After init, the current mode can be obtained via get_current_mode
    assert_eq!(get_current_mode(), Some(AppMode::Test));

    // Verify tracing can be used (console + file)
    tracing::info!("Test log message from test mode");
}
