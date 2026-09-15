//! Test mode logging: outputs to both console (pretty) and file (JSON).
//!
//! Suitable for integration testing or local debugging scenarios, balancing readability and
//! persistence.
//! The log directory is controlled via the `LOG_DIR` environment variable (default `logs`).
//! The log level is controlled via the `RUST_LOG` environment variable (default `info`).

use tracing_appender::non_blocking::WorkerGuard;
use tracing_subscriber::{Layer, prelude::*};

use crate::console_tracing::{build_console_layer, build_debug_filter};
use crate::file_tracing::{
    build_file_filter, build_file_layer, resolve_log_dir, resolve_log_prefix,
};

/// Initialize test mode logging: outputs to both console and file.
///
/// Both console and file can be controlled via the `RUST_LOG` env var (defaults to `info` when
/// not set). Note: both read the same `RUST_LOG` env var and are not fully independently
/// controlled; for differentiation, use per-target directives in `RUST_LOG`, e.g.
/// `info,my_crate=debug`.
///
/// The returned `WorkerGuard` must stay alive to ensure file logs are fully written.
/// This function calls `try_init()` and can only be called once per process.
///
/// `log_prefix` overrides the `LOG_PREFIX` environment variable when `Some`; otherwise the
/// `LOG_PREFIX` env var (default `app.log`) is used as the file name prefix.
pub fn test_tracing(crates: Vec<String>, log_prefix: Option<String>) -> anyhow::Result<WorkerGuard> {
    let log_dir = resolve_log_dir();
    let log_prefix = log_prefix.unwrap_or_else(resolve_log_prefix);

    // File layer: use build_file_filter to control the level
    let (file_layer, guard) = build_file_layer(&log_dir, &log_prefix)?;
    let file_filter = build_file_filter("info")?;

    // Console layer: use build_debug_filter to control the level
    let console_layer = build_console_layer();
    let console_filter = build_debug_filter(crates);

    tracing_subscriber::registry()
        .with(file_layer.with_filter(file_filter))
        .with(console_layer.with_filter(console_filter))
        .try_init()?;

    Ok(guard)
}
