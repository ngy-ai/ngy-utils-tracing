//! Retention integration test
//!
//! Verify that `init()` starts the background retention task and returns its handle when
//! `InitOptions::retention_interval` is set. This file exists separately because the tracing
//! subscriber can only be initialized once per process.

use ngy_utils_tracing::{AppMode, InitOptions, get_current_mode, init};
use std::time::Duration;

/// Prefix for the environment variables the crate may read in this test.
const ENV_PREFIX: &str = "NGY_TEST_";

#[test]
fn test_init_starts_retention_task() {
    // Specify production mode + a retention interval to avoid modifying process env vars (unsafe)
    let options = InitOptions::new(ENV_PREFIX)
        .mode_override(Some("production".to_string()))
        .retention_interval(Some(Duration::from_secs(3600)));

    let result = init(options).expect("init should succeed");

    assert_eq!(result.mode, AppMode::Production);
    // Background retention task should be running and its handle returned
    assert!(result.retention.is_some());
    assert!(result.guard.is_some());
    assert_eq!(get_current_mode(), Some(AppMode::Production));

    // Dropping InitResult (guard + retention handle) stops the task without panicking.
}
