//! Production mode initialization integration test
//!
//! Verify that init() uses file logging (JSON) in Production mode and returns a guard.
//! This file exists separately because the tracing subscriber can only be initialized once.

use ngy_utils_tracing::{AppMode, InitOptions, get_current_mode, init};

#[test]
fn test_full_init_production() {
    // Specify production mode via mode_override to avoid modifying process env vars (unsafe)
    let result = init(InitOptions::default().mode_override(Some("production".to_string())))
        .expect("init should succeed");

    assert_eq!(result.mode, AppMode::Production);
    // Production mode uses file logging and should return a guard
    assert!(result.guard.is_some());
    // Periodic retention is enabled by default (1 hour unless overridden/disabled via env)
    assert!(result.retention.is_some());
    // After init, the current mode can be obtained via get_current_mode
    assert_eq!(get_current_mode(), Some(AppMode::Production));

    // Verify tracing can be used (writes JSON file logs)
    tracing::info!("Test log message from production mode");

    // The result (with guard) is released at test end, ensuring file logs are flushed
}
