//! Test mode logging: outputs to both console (pretty) and file (JSON).
//!
//! Suitable for integration testing or local debugging scenarios, balancing readability and
//! persistence.
//! The log directory is controlled via the `LOG_DIR` environment variable (default `logs`).

use tracing_appender::non_blocking::WorkerGuard;
use tracing_subscriber::{EnvFilter, Layer, prelude::*};

use crate::console_tracing::{build_console_layer, debug_directives};
use crate::file_tracing::{
    build_file_layer, resolve_log_dir, resolve_log_prefix, resolve_max_log_files,
};

/// Initialize test mode logging: outputs to both console and file.
///
/// Both layers are given the **same** directive list, so a record is written to the console and to
/// the JSON file at the same level: the `crates` list raises those crates to `debug` in both
/// outputs, and `RUST_LOG` is layered on top for both (see
/// [`build_debug_filter`](crate::build_debug_filter)). Timestamps are RFC 3339 with the offset from
/// `LOG_TIME_OFFSET`, in the same format for both outputs.
///
/// The returned `WorkerGuard` must stay alive to ensure file logs are fully written.
/// This function calls `try_init()` and can only be called once per process.
///
/// `env_prefix` is prepended to every environment variable this function reads (see
/// [`InitOptions::env_prefix`](crate::InitOptions::env_prefix)). `log_dir` overrides the prefixed
/// `LOG_DIR` variable when `Some`; otherwise `LOG_DIR` (default `logs`) is used. `log_prefix`
/// overrides the prefixed `LOG_PREFIX` variable when `Some`; otherwise `LOG_PREFIX` (default
/// `app.log`) is used as the file name prefix. `max_log_files` overrides the prefixed
/// `LOG_MAX_FILES` variable when `Some`; otherwise the default `7` is used as the retention limit.
pub fn test_tracing(
    env_prefix: &str,
    crates: Vec<String>,
    log_dir: Option<String>,
    log_prefix: Option<String>,
    max_log_files: Option<usize>,
) -> anyhow::Result<WorkerGuard> {
    test_tracing_with_offset(env_prefix, crates, log_dir, log_prefix, max_log_files, None)
}

/// [`test_tracing`] with an explicit timestamp offset override; used by [`crate::init`] so that
/// [`InitOptions::time_offset`](crate::InitOptions::time_offset) reaches both layers.
pub(crate) fn test_tracing_with_offset(
    env_prefix: &str,
    crates: Vec<String>,
    log_dir: Option<String>,
    log_prefix: Option<String>,
    max_log_files: Option<usize>,
    time_offset: Option<&str>,
) -> anyhow::Result<WorkerGuard> {
    let log_dir = log_dir.unwrap_or_else(|| resolve_log_dir(env_prefix));
    let log_prefix = log_prefix.unwrap_or_else(|| resolve_log_prefix(env_prefix));
    let max_files = max_log_files.unwrap_or_else(|| resolve_max_log_files(env_prefix));
    let time_offset = crate::timestamp::resolve_time_offset(env_prefix, time_offset);

    // One directive list for both layers: otherwise the console would show the `crates` debug
    // records that the JSON file silently drops, which is exactly what is missing when a log is
    // read back after the fact.
    let directives = debug_directives(&crates);

    let (file_layer, guard) = build_file_layer(&log_dir, &log_prefix, max_files, time_offset)?;
    let console_layer = build_console_layer(time_offset);

    crate::install_subscriber(
        tracing_subscriber::registry()
            .with(file_layer.with_filter(EnvFilter::new(&directives)))
            .with(console_layer.with_filter(EnvFilter::new(&directives))),
    )?;

    Ok(guard)
}
