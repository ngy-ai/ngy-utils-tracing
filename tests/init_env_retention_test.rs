//! Environment-driven retention integration test
//!
//! Verify that `init()` reads the prefixed `LOG_RETENTION_INTERVAL_SECONDS` env var (seconds) when
//! `InitOptions::retention_interval` is not set, and starts the background task, while an
//! unprefixed `LOG_RETENTION_INTERVAL_SECONDS` is ignored.
//! This file exists separately because the tracing subscriber can only be initialized
//! once per process.

use ngy_utils_tracing::{AppMode, InitOptions, get_current_mode, init};

/// Prefix for the variables this test sets; the crate never reads unprefixed names.
const ENV_PREFIX: &str = "NGY_TEST_";

#[test]
fn test_init_reads_retention_from_env() {
    // SAFETY: From Rust 2024, modifying process env vars is unsafe. This test file is the only one
    // that sets LOG_RETENTION_INTERVAL_SECONDS, and init() is called once here, so there is no
    // concurrent access to env vars by other tests in this binary.
    unsafe {
        std::env::set_var(
            format!("{ENV_PREFIX}LOG_RETENTION_INTERVAL_SECONDS"),
            "3600",
        );
        // Decoy: another program owning the unprefixed name must not influence us.
        std::env::set_var("LOG_RETENTION_INTERVAL_SECONDS", "0");
    }

    // Force production mode so the test does not depend on APP_MODE from the environment or a
    // `.env` file; the retention interval still comes from the env var (`retention_interval`
    // is left as `None`).
    let result = init(InitOptions::new(ENV_PREFIX).mode_override(Some("production".to_string())))
        .expect("init should succeed");

    assert_eq!(result.mode, AppMode::Production);
    // The prefixed value (3600) wins; the unprefixed decoy (`0`, which disables retention) is not read.
    assert!(result.retention.is_some());
    assert_eq!(get_current_mode(), Some(AppMode::Production));

    // SAFETY: Same as above; restore the env vars set by this test.
    unsafe {
        std::env::remove_var(format!("{ENV_PREFIX}LOG_RETENTION_INTERVAL_SECONDS"));
        std::env::remove_var("LOG_RETENTION_INTERVAL_SECONDS");
    }
}
