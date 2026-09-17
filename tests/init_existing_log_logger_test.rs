//! Integration test: `init()` must tolerate an already installed `log` logger.
//!
//! Applications commonly install a `log` logger (e.g. `env_logger`) before configuring tracing.
//! That must not fail the whole initialization — only the `log` compatibility layer is skipped.
//! This file exists separately because the `log` logger and the tracing subscriber are both
//! process-wide and can be set only once.

use ngy_utils_tracing::{AppMode, InitOptions, get_current_mode, init};

/// Prefix for the environment variables the crate may read in this test.
const ENV_PREFIX: &str = "NGY_TEST_";

/// Minimal `log` logger, standing in for whatever the host application installed first.
struct ExistingLogger;

impl log::Log for ExistingLogger {
    fn enabled(&self, _metadata: &log::Metadata<'_>) -> bool {
        true
    }

    fn log(&self, _record: &log::Record<'_>) {}

    fn flush(&self) {}
}

#[test]
fn init_succeeds_when_a_log_logger_is_already_installed() {
    // Take ownership of the `log` crate first, the way an application using `env_logger` would.
    log::set_boxed_logger(Box::new(ExistingLogger)).expect("no other test installed a log logger");
    log::set_max_level(log::LevelFilter::Info);

    // The tracing subscriber must still be installed; only the `log` bridge is skipped.
    let result = init(InitOptions::new(ENV_PREFIX).mode_override(Some("test".to_string())))
        .expect("an existing `log` logger must not make init() fail");

    assert_eq!(result.mode, AppMode::Test);
    assert!(result.guard.is_some());
    assert_eq!(get_current_mode(), Some(AppMode::Test));

    // The application's own `log` logger keeps working, and tracing still works alongside it.
    log::info!("record handled by the application's logger");
    tracing::info!("Test log message written despite the existing `log` logger");
}
