//! # ngy-utils-tracing
//!
//! A logging initialization utility library built on [`tracing`], which automatically selects the
//! log output based on the "application mode" ([`AppMode`]):
//!
//! - `Development`: Console logging (pretty format, with file name and line number, timezone UTC+8)
//! - `Test`: Console (pretty) + File (JSON)
//! - `Production`: File logging (JSON, daily rotation)
//!
//! [`init`] performs one-time initialization of dotenv and tracing; repeated calls within the
//! process return a clear error. After initialization, use [`get_current_mode`] to query the
//! current mode.
//!
//! See the [README](https://crates.io/crates/ngy-utils-tracing) for mode descriptions and the full API.

mod app_mode;
mod console_tracing;
mod file_tracing;
mod init;
mod test_tracing;

pub use app_mode::AppMode;
pub use console_tracing::{build_debug_filter, console_tracing};
pub use file_tracing::{
    DEFAULT_RETENTION_INTERVAL, LogRetentionHandle, build_file_filter, cleanup_old_logs,
    file_tracing, resolve_log_dir, resolve_log_prefix, resolve_max_log_files,
    resolve_retention_interval, start_log_retention,
};
pub use init::{InitOptions, InitResult, get_current_mode, init};
pub use test_tracing::test_tracing;

// Test-only: serialize all environment variable access across the lib test binary to avoid
// data races (unsafe) causing UB. Tests in both console_tracing and file_tracing modules use it.
#[cfg(test)]
pub(crate) static TEST_ENV_MUTEX: std::sync::Mutex<()> = std::sync::Mutex::new(());
