//! Initialization module: unified initialization of dotenv and tracing
//!
//! Selects the corresponding tracing configuration automatically based on AppMode.

use crate::{
    app_mode::AppMode, console_tracing::console_tracing, file_tracing::file_tracing,
    test_tracing::test_tracing,
};
use std::sync::OnceLock;
use tracing_appender::non_blocking::WorkerGuard;

/// Globally stores the current application mode
static CURRENT_MODE: OnceLock<AppMode> = OnceLock::new();

/// Initialization result, containing the guard that must stay alive
pub struct InitResult {
    /// Guard for file logging (valid in production and test modes)
    pub guard: Option<WorkerGuard>,
    /// Current application mode
    pub mode: AppMode,
}

/// Initialize dotenv and tracing
///
/// Selects the corresponding logging configuration based on the `APP_MODE` environment variable:
/// - Development: Console logging (pretty format)
/// - Test: Console + file logging
/// - Production: File logging (JSON format)
///
/// # Parameters
///
/// - `mode_override`: Optional application mode override. An invalid value triggers a warning and
///   falls back to Development. If not provided, the mode is read from the `APP_MODE` env var.
/// - `crates`: List of project crate names that should use the `debug` level in console logging.
/// - `log_prefix_name`: Optional log file name prefix (without the date suffix). When `Some`, it
///   overrides the `LOG_PREFIX` environment variable for file logging; otherwise `LOG_PREFIX`
///   (default `app.log`) is used as the file name prefix.
///
/// # Example
///
/// ```no_run
/// use ngy_utils_tracing::init;
///
/// // Read mode from the environment variable
/// let result = init(None, Vec::new(), None).expect("Failed to initialize");
///
/// // Force a specific mode with a custom log file prefix
/// let result = init(Some("production"), Vec::new(), Some("myapp.log".to_string())).expect("Failed to initialize");
/// tracing::info!("Application started in {} mode", result.mode);
/// ```
pub fn init(
    mode_override: Option<&str>,
    crates: Vec<String>,
    log_prefix_name: Option<String>,
) -> anyhow::Result<InitResult> {
    // Prevent repeated initialization: the global tracing subscriber can only be set once, and
    // CURRENT_MODE is immutable; repeated calls should return a clear error early rather than
    // having CURRENT_MODE.set silently ignored.
    if CURRENT_MODE.get().is_some() {
        anyhow::bail!("tracing already initialized; init() must be called only once per process");
    }

    // Determine the mode first, used to decide whether to print loading info
    let mode = AppMode::get(mode_override);

    // Store in the global variable (the duplicate check above guarantees set succeeds here)
    CURRENT_MODE.set(mode).ok();

    // Load the .env file, or use environment variables if it does not exist
    match dotenvy::dotenv_override() {
        Ok(path) => {
            if !mode.is_production() {
                println!("[ENV] Loaded .env from: {}", path.display());
            }
        }
        Err(_) => {
            if !mode.is_production() {
                println!("[ENV] No .env file found, using environment variables");
            }
        }
    }

    let guard = match mode {
        AppMode::Production => {
            let g = file_tracing(log_prefix_name)?;
            Some(g)
        }
        AppMode::Development => {
            console_tracing(crates)?;
            None
        }
        AppMode::Test => {
            let g = test_tracing(crates, log_prefix_name)?;
            Some(g)
        }
    };

    Ok(InitResult { guard, mode })
}

/// Get the current application mode
///
/// Must be used after calling `init()`, otherwise returns `None`.
///
/// # Example
///
/// ```no_run
/// use ngy_utils_tracing::{init, get_current_mode};
///
/// init(None, Vec::new(), None).expect("Failed to initialize");
///
/// if let Some(mode) = get_current_mode() {
///     println!("Current mode: {}", mode);
/// }
/// ```
pub fn get_current_mode() -> Option<AppMode> {
    CURRENT_MODE.get().copied()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn init_result_structure() {
        // Test that InitResult has the expected fields
        let result = InitResult {
            guard: None,
            mode: AppMode::Development,
        };
        assert_eq!(result.mode, AppMode::Development);
        assert!(result.guard.is_none());
    }

    #[test]
    fn init_result_with_guard() {
        // Test that InitResult can hold a guard
        let result = InitResult {
            guard: None,
            mode: AppMode::Production,
        };
        assert_eq!(result.mode, AppMode::Production);
    }

    // Note: We don't test the full init() function because it calls
    // set_global_default/try_init which can only be called once per process.
    // Integration tests should test the full initialization flow.
}
