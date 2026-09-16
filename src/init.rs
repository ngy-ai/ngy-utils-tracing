//! Initialization module: unified initialization of dotenv and tracing
//!
//! Selects the corresponding tracing configuration automatically based on AppMode.

use crate::{
    app_mode::AppMode,
    console_tracing::console_tracing,
    file_tracing::{
        LogRetentionHandle, file_tracing, resolve_retention_interval_or_default,
        start_log_retention,
    },
    test_tracing::test_tracing,
};
use std::sync::OnceLock;
use tracing_appender::non_blocking::WorkerGuard;

/// Globally stores the current application mode
static CURRENT_MODE: OnceLock<AppMode> = OnceLock::new();

/// Initialization result, containing the handles that must stay alive
pub struct InitResult {
    /// Guard for file logging (valid in production and test modes)
    pub guard: Option<WorkerGuard>,
    /// Current application mode
    pub mode: AppMode,
    /// Handle for the optional background log-retention task. Present only when
    /// [`InitOptions::retention_interval`] was set. Keep `InitResult` alive for the process
    /// lifetime to keep the task running; dropping it stops the background thread.
    pub retention: Option<LogRetentionHandle>,
}

/// Configuration for [`init`].
///
/// Every field is optional. When a field is `None`, the corresponding environment
/// variable (or built-in default) is used as a fallback, so callers only need to
/// set the values they care about.
///
/// # Examples
///
/// ```no_run
/// use ngy_utils_tracing::InitOptions;
///
/// // Use all defaults (mode from APP_MODE, logs/ directory, app.log prefix, keep 7 files)
/// let options = InitOptions::default();
///
/// // Chainable builder for a production setup with a custom prefix and retention
/// let options = InitOptions::default()
///     .mode_override(Some("production".to_string()))
///     .log_dir(Some("logs".to_string()))
///     .log_prefix(Some("myapp.log".to_string()))
///     .max_log_files(Some(7))
///     .retention_interval(Some(std::time::Duration::from_secs(3600)));
/// ```
#[derive(Default)]
pub struct InitOptions {
    /// Optional application mode override. An invalid value triggers a warning and
    /// falls back to Production. If `None`, the mode is read from `mode_env_var`
    /// (default `APP_MODE`).
    pub mode_override: Option<String>,
    /// List of project crate names that should use the `debug` level in console logging.
    pub crates: Vec<String>,
    /// Optional log directory override. When `Some`, overrides the `LOG_DIR` environment
    /// variable; when `None`, `LOG_DIR` (default `logs`) is used.
    pub log_dir: Option<String>,
    /// Optional log file name prefix override (without the date suffix). When `Some`, overrides
    /// the `LOG_PREFIX` environment variable; when `None`, `LOG_PREFIX` (default `app.log`) is used.
    pub log_prefix: Option<String>,
    /// Optional max number of retained log files (including the file created for the current
    /// day). When `Some`, overrides `LOG_MAX_FILES`; when `None`, `LOG_MAX_FILES` (default `7`)
    /// is used. Older files beyond this are deleted at init, and a slot is reserved for the
    /// current day's file so the directory never holds more than `max_log_files` files.
    pub max_log_files: Option<usize>,
    /// Optional mode env var name. When `Some`, that variable is read instead of the default
    /// `APP_MODE`; when `None`, `APP_MODE` is used.
    pub mode_env_var: Option<String>,
    /// Optional background retention interval. When `Some`, `init` spawns a background task that
    /// periodically enforces `max_log_files` (useful for long-running processes spanning many
    /// daily rotations). When `None`, the `LOG_RETENTION_INTERVAL_SECONDS` environment variable is
    /// used (interpreted as seconds); if that variable is unset or invalid,
    /// [`crate::DEFAULT_RETENTION_INTERVAL`] (1 hour) is used, so periodic cleanup is **enabled by
    /// default**. Set the variable to `0` to disable the background task explicitly. The task
    /// applies to `Production` and `Test` modes (the ones that write files) and uses the same
    /// `log_dir` / `log_prefix` / `max_log_files` resolution as file logging.
    pub retention_interval: Option<std::time::Duration>,
}

impl InitOptions {
    /// Create an `InitOptions` with all fields unset (relying on env vars / defaults).
    pub fn new() -> Self {
        Self::default()
    }

    /// Set the optional application mode override.
    pub fn mode_override(mut self, mode_override: Option<String>) -> Self {
        self.mode_override = mode_override;
        self
    }

    /// Set the list of crate names that use the `debug` level in console logging.
    pub fn crates(mut self, crates: Vec<String>) -> Self {
        self.crates = crates;
        self
    }

    /// Set the optional log directory override.
    pub fn log_dir(mut self, log_dir: Option<String>) -> Self {
        self.log_dir = log_dir;
        self
    }

    /// Set the optional log file name prefix override.
    pub fn log_prefix(mut self, log_prefix: Option<String>) -> Self {
        self.log_prefix = log_prefix;
        self
    }

    /// Set the optional max number of retained log files.
    pub fn max_log_files(mut self, max_log_files: Option<usize>) -> Self {
        self.max_log_files = max_log_files;
        self
    }

    /// Set the optional mode environment variable name.
    pub fn mode_env_var(mut self, mode_env_var: Option<String>) -> Self {
        self.mode_env_var = mode_env_var;
        self
    }

    /// Set the optional background retention interval. When `Some`, `init` starts a background task
    /// that periodically trims old log files; when `None`, the environment variable / default
    /// resolution applies (see [`InitOptions::retention_interval`]), which enables periodic cleanup
    /// by default.
    pub fn retention_interval(mut self, retention_interval: Option<std::time::Duration>) -> Self {
        self.retention_interval = retention_interval;
        self
    }
}

/// Initialize dotenv and tracing
///
/// Selects the corresponding logging configuration based on the application mode:
/// - Development: Console logging (pretty format)
/// - Test: Console + file logging
/// - Production: File logging (JSON format)
///
/// All settings are supplied via the [`InitOptions`] struct; any field left as `None`
/// falls back to its environment variable (or built-in default). See [`InitOptions`] for
/// the list of supported fields and their env-var fallbacks.
///
/// A background task that periodically enforces `max_log_files` is spawned for `Production` and
/// `Test` modes; its handle is returned in [`InitResult::retention`]. The interval comes from
/// [`InitOptions::retention_interval`] or the `LOG_RETENTION_INTERVAL_SECONDS` environment
/// variable, defaulting to [`crate::DEFAULT_RETENTION_INTERVAL`] (1 hour) so retention runs by
/// default; set that variable to `0` to disable the background task. Keep `InitResult` alive for
/// the process lifetime to keep both the file `guard` and the retention task running.
///
/// # Example
///
/// ```no_run
/// use ngy_utils_tracing::{InitOptions, init};
/// use std::time::Duration;
///
/// // Read mode from the environment variable, keep all other defaults
/// let result = init(InitOptions::default()).expect("Failed to initialize");
///
/// // Force a specific mode with a custom prefix, retention, and a 1h background trim
/// let options = InitOptions::default()
///     .mode_override(Some("production".to_string()))
///     .log_prefix(Some("myapp.log".to_string()))
///     .max_log_files(Some(14))
///     .retention_interval(Some(Duration::from_secs(3600)));
/// let result = init(options).expect("Failed to initialize");
/// tracing::info!("Application started in {} mode", result.mode);
/// ```
pub fn init(options: InitOptions) -> anyhow::Result<InitResult> {
    // Prevent repeated initialization: the global tracing subscriber can only be set once, and
    // CURRENT_MODE is immutable; repeated calls should return a clear error early rather than
    // having CURRENT_MODE.set silently ignored.
    if CURRENT_MODE.get().is_some() {
        anyhow::bail!("tracing already initialized; init() must be called only once per process");
    }

    // Determine the mode first, used to decide whether to print loading info
    let mode = AppMode::get(
        options.mode_override,
        options.mode_env_var.as_deref().unwrap_or("APP_MODE"),
    );

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

    // Clone the file-related options so they can be reused for the retention task below.
    let log_dir_opt = options.log_dir.clone();
    let log_prefix_opt = options.log_prefix.clone();
    let max_files_opt = options.max_log_files;
    // Explicit option wins; otherwise fall back to the LOG_RETENTION_INTERVAL_SECONDS env var
    // (seconds). When that variable is unset/invalid, DEFAULT_RETENTION_INTERVAL is used so the
    // periodic cleanup is enabled by default; an explicit `0` disables the background task.
    let retention_interval = options
        .retention_interval
        .or_else(resolve_retention_interval_or_default);

    let guard = match mode {
        AppMode::Production => {
            let g = file_tracing(log_dir_opt.clone(), log_prefix_opt.clone(), max_files_opt)?;
            Some(g)
        }
        AppMode::Development => {
            console_tracing(options.crates)?;
            None
        }
        AppMode::Test => {
            let g = test_tracing(
                options.crates,
                log_dir_opt.clone(),
                log_prefix_opt.clone(),
                max_files_opt,
            )?;
            Some(g)
        }
    };

    // Start the background retention task only for file-producing modes when an interval is set.
    let retention = if matches!(mode, AppMode::Production | AppMode::Test) {
        match retention_interval {
            Some(interval) => Some(start_log_retention(
                log_dir_opt,
                log_prefix_opt,
                max_files_opt,
                interval,
            )?),
            None => None,
        }
    } else {
        None
    };

    // Publish the mode only after every step succeeded: a failed init (e.g. the subscriber could
    // not be set) must not leave CURRENT_MODE populated, otherwise a later retry would be
    // rejected as "already initialized" while no subscriber was ever installed.
    CURRENT_MODE.set(mode).ok();

    Ok(InitResult {
        guard,
        mode,
        retention,
    })
}

/// Get the current application mode
///
/// Must be used after calling `init()`, otherwise returns `None`.
///
/// # Example
///
/// ```no_run
/// use ngy_utils_tracing::{InitOptions, init, get_current_mode};
///
/// init(InitOptions::default()).expect("Failed to initialize");
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
            retention: None,
        };
        assert_eq!(result.mode, AppMode::Development);
        assert!(result.guard.is_none());
        assert!(result.retention.is_none());
    }

    #[test]
    fn init_result_with_guard() {
        // Test that InitResult can hold a guard
        let result = InitResult {
            guard: None,
            mode: AppMode::Production,
            retention: None,
        };
        assert_eq!(result.mode, AppMode::Production);
    }

    // Note: We don't test the full init() function because it calls
    // set_global_default/try_init which can only be called once per process.
    // Integration tests should test the full initialization flow.
}
