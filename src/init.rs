//! Initialization module: unified initialization of dotenv and tracing
//!
//! Selects the corresponding tracing configuration automatically based on AppMode.

use crate::{
    app_mode::AppMode, console_tracing::console_tracing_with_offset,
    file_tracing::file_tracing_with_offset, test_tracing::test_tracing_with_offset,
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
/// // Chainable builder for a production setup with explicit paths and a retention limit
/// let options = InitOptions::new("NGY_")
///     .mode_override(Some("production".to_string()))
///     .log_dir(Some("logs".to_string()))
///     .log_prefix(Some("myapp.log".to_string()))
///     .max_log_files(Some(7));
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
    /// Optional max number of retained log files, including the file of the current day. When
    /// `Some`, overrides `LOG_MAX_FILES`; when `None`, `LOG_MAX_FILES` is read and its value falls
    /// back to [`crate::DEFAULT_MAX_LOG_FILES`]. Values below `1` are treated as `1`, so the file
    /// being written is never removed. The directory is trimmed to this many files whenever the
    /// writer rolls over to a new day — including the first record after a restart, which opens the
    /// current day's file — rather than when `init` runs.
    pub max_log_files: Option<usize>,
    /// Optional mode env var name. When `Some`, that name is used **verbatim**, without the
    /// [`env_prefix`](InitOptions::env_prefix); when `None`, the mode is read from the prefixed
    /// `APP_MODE` variable.
    pub mode_env_var: Option<String>,
    /// Optional UTC offset for log timestamps, written as `+08:00`, `-05:30`, `UTC` or `Z`. When
    /// `Some`, overrides the `LOG_TIME_OFFSET` environment variable; when `None`, the prefixed
    /// `LOG_TIME_OFFSET` variable is read, and if that is unset as well the **machine's own time
    /// zone** is used, so a program that configures nothing still reports local time.
    /// [`crate::DEFAULT_TIME_OFFSET`] is only the last resort for a machine that cannot report a
    /// zone. Both the console and the file layer print RFC 3339 with that offset, so a record reports
    /// the same wall-clock time in the same format wherever it is written. The offset also decides
    /// the date in the log file name (`{prefix}.{YYYY-MM-DD}`), so a record and the name of the file
    /// holding it always agree. It is resolved once, at `init`, so a daylight-saving switch mid-run
    /// does not move it. An invalid value triggers a warning and is skipped, so resolution continues
    /// with the next source.
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
/// In `Production` and `Test` mode the file writer also keeps the log directory bounded: it trims
/// it to [`InitOptions::max_log_files`] files every time it rolls over to a new day (see
/// [`InitOptions::time_offset`] for the date in the file name). Keep `InitResult` alive for the
/// process lifetime so the file `guard` can flush what is still buffered.
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
///
/// // Read the mode from the `NGY_APP_MODE` variable, keep all other defaults
/// let result = init(InitOptions::new("NGY_")).expect("Failed to initialize");
///
/// // Force a specific mode with explicit paths and a 14-file retention limit
/// let options = InitOptions::new("NGY_")
///     .mode_override(Some("production".to_string()))
///     .log_prefix(Some("myapp.log".to_string()))
///     .max_log_files(Some(14));
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

    // Every layer shares one timestamp offset; the explicit option wins over the prefixed
    // LOG_TIME_OFFSET variable.
    let time_offset = options.time_offset.as_deref();

    let guard = match mode {
        // The file layers resolve the log settings themselves (explicit option first, environment
        // second) and trim the directory to the retention limit when they roll over to a new day.
        AppMode::Production => Some(file_tracing_with_offset(
            env_prefix,
            options.log_dir,
            options.log_prefix,
            options.max_log_files,
            time_offset,
        )?),
        AppMode::Development => {
            console_tracing_with_offset(env_prefix, options.crates, time_offset)?;
            None
        }
        AppMode::Test => Some(test_tracing_with_offset(
            env_prefix,
            options.crates,
            options.log_dir,
            options.log_prefix,
            options.max_log_files,
            time_offset,
        )?),
    };

    // Publish the mode only after every step succeeded: a failed init (e.g. the subscriber could
    // not be set) must not leave CURRENT_MODE populated, otherwise a later retry would be
    // rejected as "already initialized" while no subscriber was ever installed.
    CURRENT_MODE.set(mode).ok();

    Ok(InitResult { guard, mode })
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
