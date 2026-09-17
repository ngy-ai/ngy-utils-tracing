//! Initialization module: unified initialization of dotenv and tracing
//!
//! Selects the corresponding tracing configuration automatically based on AppMode.

use crate::{
    app_mode::AppMode,
    console_tracing::console_tracing_with_offset,
    file_tracing::{file_tracing_with_offset, start_log_retention},
    retention::{LogRetentionHandle, resolve_retention_interval_or_default},
    test_tracing::test_tracing_with_offset,
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
    /// Handle for the optional background log-retention task. Present in `Production` / `Test` when
    /// a retention interval is resolved (see [`InitOptions::retention_interval`]), and `None`
    /// otherwise. Keep `InitResult` alive for the process lifetime to keep the task running;
    /// dropping it stops the background thread.
    pub retention: Option<LogRetentionHandle>,
}

/// Configuration for [`init`].
///
/// This struct is the single source of truth for the option ↔ environment variable ↔ default
/// mapping: every field below names the variable it overrides and the fallback used when it is
/// `None` (the built-in defaults are the `DEFAULT_*` constants). `README.md` only repeats a short
/// summary of the table.
///
/// Every variable is looked up as [`env_prefix`](InitOptions::env_prefix) + the documented name:
/// with the prefix `NGY_`, the log directory comes from `NGY_LOG_DIR` and **never** from the
/// unprefixed `LOG_DIR`. That keeps unrelated programs — which are free to use `LOG_DIR`,
/// `APP_MODE` and friends for their own purposes — from silently changing this crate's behaviour.
/// `RUST_LOG` is the single exception: it stays unprefixed because the whole Rust logging
/// ecosystem shares it.
///
/// Apart from `env_prefix`, every field is optional, so callers only need to set the values they
/// care about.
///
/// # Examples
///
/// ```no_run
/// use ngy_utils_tracing::InitOptions;
///
/// // Use all defaults: environment configuration is read from the `NGY_*` variables
/// let options = InitOptions::new("NGY_");
///
/// // Chainable builder for a production setup with explicit paths and retention
/// let options = InitOptions::new("NGY_")
///     .mode_override(Some("production".to_string()))
///     .log_dir(Some("logs".to_string()))
///     .log_prefix(Some("myapp.log".to_string()))
///     .max_log_files(Some(7))
///     .retention_interval(Some(std::time::Duration::from_secs(3600)));
/// ```
pub struct InitOptions {
    /// **Required** prefix for every environment variable this crate reads, e.g. `"NGY_"` makes the
    /// log directory come from `NGY_LOG_DIR`. The prefix is used verbatim (plain concatenation), so
    /// include any separator yourself. It must not be empty: an empty prefix would read the
    /// unprefixed `LOG_DIR` / `APP_MODE` again and reintroduce exactly the collisions this field
    /// exists to prevent, which is why [`InitOptions::new`] rejects it. `RUST_LOG` is never
    /// prefixed.
    pub env_prefix: String,
    /// Optional application mode override. An invalid value triggers a warning and is skipped, so
    /// the mode falls back to `mode_env_var`. If `None`, the mode is read from `mode_env_var`
    /// **after `.env` has been loaded**, so a value defined in `.env` is honoured (see
    /// [`AppMode::get`] for the full resolution order).
    pub mode_override: Option<String>,
    /// List of project crate names that should use the `debug` level in console logging; every
    /// other crate falls back to [`crate::DEFAULT_LOG_LEVEL`]. The `RUST_LOG` environment variable
    /// is layered on top: a directive there can override an individual crate or replace the global
    /// fallback with a bare level such as `warn`, but it never discards the `debug` level of a
    /// crate it does not mention (see [`crate::build_debug_filter`]). In `Test` mode the file layer
    /// receives the same directives, so the JSON file holds those records as well; in `Production`
    /// mode there is no console layer and this field has no effect. A name that cannot form a
    /// valid directive is ignored with a warning on stderr.
    pub crates: Vec<String>,
    /// Optional log directory override. When `Some`, overrides the `LOG_DIR` environment variable;
    /// when `None`, `LOG_DIR` is read and its value falls back to [`crate::DEFAULT_LOG_DIR`].
    pub log_dir: Option<String>,
    /// Optional log file name prefix override (without the date suffix). When `Some`, overrides the
    /// `LOG_PREFIX` environment variable; when `None`, `LOG_PREFIX` is read and its value falls
    /// back to [`crate::DEFAULT_LOG_PREFIX`]. Daily files are named `{prefix}.{YYYY-MM-DD}`.
    pub log_prefix: Option<String>,
    /// Optional max number of retained log files (including the file created for the current day).
    /// When `Some`, overrides `LOG_MAX_FILES`; when `None`, `LOG_MAX_FILES` is read and its value
    /// falls back to [`crate::DEFAULT_MAX_LOG_FILES`]. Older files beyond this are deleted at init,
    /// and a slot is reserved for the current day's file so the directory never holds more than
    /// `max_log_files` files.
    pub max_log_files: Option<usize>,
    /// Optional mode env var name. When `Some`, that name is used **verbatim**, without the
    /// [`env_prefix`](InitOptions::env_prefix); when `None`, the mode is read from the prefixed
    /// `APP_MODE` variable.
    pub mode_env_var: Option<String>,
    /// Optional background retention interval. When `Some`, `init` spawns a background task that
    /// periodically enforces `max_log_files` (useful for long-running processes spanning many
    /// daily rotations). When `None`, the `LOG_RETENTION_INTERVAL_SECONDS` environment variable is
    /// read and interpreted as seconds; if it is unset or invalid,
    /// [`crate::DEFAULT_RETENTION_INTERVAL`] is used, so periodic cleanup is **enabled by
    /// default**. Set the variable to `0` to disable the background task explicitly. The task
    /// applies to `Production` and `Test` modes (the ones that write files) and uses the same
    /// `log_dir` / `log_prefix` / `max_log_files` resolution as file logging.
    pub retention_interval: Option<std::time::Duration>,
    /// Optional UTC offset for log timestamps, written as `+08:00`, `-05:30`, `UTC` or `Z`. When
    /// `Some`, overrides the `LOG_TIME_OFFSET` environment variable; when `None`, `LOG_TIME_OFFSET`
    /// is read and its value falls back to [`crate::DEFAULT_TIME_OFFSET`]. Both the console and the
    /// file layer print RFC 3339 with that offset, so a record reports the same wall-clock time in
    /// the same format wherever it is written. An invalid value triggers a warning and is skipped,
    /// so resolution continues with the environment variable.
    pub time_offset: Option<String>,
}

impl InitOptions {
    /// Create an `InitOptions` whose environment variables use `env_prefix`, relying on the
    /// environment / built-in defaults for every other field.
    ///
    /// # Panics
    ///
    /// Panics when `env_prefix` is empty or whitespace only: an empty prefix would read the
    /// unprefixed `LOG_DIR` / `APP_MODE`, letting an unrelated program change this crate's
    /// behaviour.
    pub fn new(env_prefix: impl Into<String>) -> Self {
        let env_prefix = env_prefix.into();
        assert!(
            !env_prefix.trim().is_empty(),
            "InitOptions::new requires a non-empty env_prefix, e.g. \"NGY_\""
        );

        Self {
            env_prefix,
            mode_override: None,
            crates: Vec::new(),
            log_dir: None,
            log_prefix: None,
            max_log_files: None,
            mode_env_var: None,
            retention_interval: None,
            time_offset: None,
        }
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

    /// Set the optional UTC offset used for log timestamps (e.g. `+08:00`, `UTC`).
    pub fn time_offset(mut self, time_offset: Option<String>) -> Self {
        self.time_offset = time_offset;
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
/// All settings come from [`InitOptions`], which documents every field, the environment variable
/// it overrides and the default it falls back to. Apart from `RUST_LOG`, every variable is read
/// with the required [`InitOptions::env_prefix`], so unrelated programs cannot affect the outcome.
///
/// Mode resolution follows [`AppMode::get`]: [`InitOptions::mode_override`] → the variable named by
/// [`InitOptions::mode_env_var`] (the prefixed `APP_MODE` by default) → the default mode. `.env` is
/// loaded **before** the mode is resolved, so a prefixed `APP_MODE` defined in `.env` is honoured
/// when no explicit override is given.
///
/// A background task that periodically enforces `max_log_files` runs for `Production` and `Test`;
/// see [`InitOptions::retention_interval`] and [`InitResult::retention`]. Keep `InitResult` alive
/// for the process lifetime to keep both the file `guard` and the retention task running.
///
/// # Errors
///
/// Returns an error if `init` was already called in this process, or if a global tracing
/// subscriber was installed by someone else — in both cases the configuration cannot be applied,
/// and staying silent would leave the caller logging into nothing.
///
/// An already installed `log` logger is deliberately **not** an error: the `log` compatibility
/// layer is then skipped with a warning on stderr, while the tracing subscriber is still
/// installed.
///
/// # Example
///
/// ```no_run
/// use ngy_utils_tracing::{InitOptions, init};
/// use std::time::Duration;
///
/// // Read the mode from the `NGY_APP_MODE` variable, keep all other defaults
/// let result = init(InitOptions::new("NGY_")).expect("Failed to initialize");
///
/// // Force a specific mode with explicit paths and a 1h background trim
/// let options = InitOptions::new("NGY_")
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

    // Load the .env file *before* resolving the mode: when no explicit override is given the mode
    // is read from `mode_env_var`, and `.env` must already be applied for an `APP_MODE` defined
    // there to be honoured. The result is kept so the loading message can be printed once the mode
    // (which decides whether to stay quiet) is known.
    let dotenv_result = dotenvy::dotenv_override();

    // Every variable this crate reads is prefixed (see `InitOptions::env_prefix`), so a program
    // that happens to use `APP_MODE` or `LOG_DIR` for its own purposes cannot affect us.
    let env_prefix = options.env_prefix.as_str();

    // Mode precedence: explicit `mode_override` > `mode_env_var` (after `.env` is applied)
    // > `Production` default. An explicit `mode_env_var` is used verbatim.
    let default_mode_env_var = crate::env_var_name(env_prefix, "APP_MODE");
    let mode = AppMode::get(
        options.mode_override,
        options
            .mode_env_var
            .as_deref()
            .unwrap_or(&default_mode_env_var),
    );

    // Report how the environment was loaded; production stays quiet.
    if !mode.is_production() {
        match dotenv_result {
            Ok(path) => println!("[ENV] Loaded .env from: {}", path.display()),
            Err(_) => println!("[ENV] No .env file found, using environment variables"),
        }
    }

    // Clone the file-related options so they can be reused for the retention task below.
    let log_dir_opt = options.log_dir.clone();
    let log_prefix_opt = options.log_prefix.clone();
    let max_files_opt = options.max_log_files;
    // Explicit option wins; otherwise fall back to the prefixed LOG_RETENTION_INTERVAL_SECONDS env
    // var (seconds). When that variable is unset/invalid, DEFAULT_RETENTION_INTERVAL is used so the
    // periodic cleanup is enabled by default; an explicit `0` disables the background task.
    let retention_interval = options
        .retention_interval
        .or_else(|| resolve_retention_interval_or_default(env_prefix));

    // Every layer shares one timestamp offset; the explicit option wins over the prefixed
    // LOG_TIME_OFFSET variable.
    let time_offset = options.time_offset.as_deref();

    let guard = match mode {
        AppMode::Production => {
            let g = file_tracing_with_offset(
                env_prefix,
                log_dir_opt.clone(),
                log_prefix_opt.clone(),
                max_files_opt,
                time_offset,
            )?;
            Some(g)
        }
        AppMode::Development => {
            console_tracing_with_offset(env_prefix, options.crates, time_offset)?;
            None
        }
        AppMode::Test => {
            let g = test_tracing_with_offset(
                env_prefix,
                options.crates,
                log_dir_opt.clone(),
                log_prefix_opt.clone(),
                max_files_opt,
                time_offset,
            )?;
            Some(g)
        }
    };

    // Start the background retention task only for file-producing modes when an interval is set.
    let retention = if matches!(mode, AppMode::Production | AppMode::Test) {
        match retention_interval {
            Some(interval) => Some(start_log_retention(
                env_prefix,
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
/// init(InitOptions::new("NGY_")).expect("Failed to initialize");
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
