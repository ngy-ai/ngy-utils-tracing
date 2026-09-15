//! Console logging initialization: pretty-format output with colors, file name, and line number.
//!
//! Default timezone is UTC+8, time format `YYYY-MM-DD HH:MM:SS`.
//! Log level is controlled via the `RUST_LOG` environment variable (default `info`).

use time::{UtcOffset, macros::format_description};
use tracing_subscriber::{EnvFilter, Layer, filter::filter_fn, fmt::time::OffsetTime, prelude::*};

/// Default timezone offset (UTC+8)
const DEFAULT_UTC_OFFSET_HOURS: i8 = 8;

/// Build the project-level log filter
///
/// Project crates passed via the `crates` parameter by the caller use the `debug` level,
/// other dependency crates use the `info` level. This can be overridden entirely via the
/// `RUST_LOG` environment variable.
///
/// # Example
///
/// ```no_run
/// use ngy_utils_tracing::build_debug_filter;
///
/// let filter = build_debug_filter(Vec::new());
/// // The generated filter is similar to: agent=debug,provider=debug,tools=debug,...,info
/// ```
pub fn build_debug_filter(crates: Vec<String>) -> EnvFilter {
    // Prefer the environment variable
    if let Ok(filter) = EnvFilter::try_from_default_env() {
        return filter;
    }

    // Build the debug-level directive for project crates
    let mut directives: Vec<String> = crates
        .iter()
        .map(|crate_name| format!("{}=debug", crate_name))
        .collect();

    // Other crates default to info level
    directives.push("info".to_string());

    let directive_str = directives.join(",");
    EnvFilter::new(directive_str)
}

/// Build the console log layer (pretty format)
pub fn build_console_layer<S>() -> impl Layer<S>
where
    S: tracing::Subscriber + for<'a> tracing_subscriber::registry::LookupSpan<'a>,
{
    let offset = UtcOffset::from_hms(DEFAULT_UTC_OFFSET_HOURS, 0, 0).unwrap_or(UtcOffset::UTC);
    let timer = OffsetTime::new(
        offset,
        format_description!("[year]-[month]-[day] [hour]:[minute]:[second]"),
    );

    tracing_subscriber::fmt::layer()
        .pretty()
        .with_ansi(true)
        .with_file(true)
        .with_line_number(true)
        .with_writer(std::io::stdout)
        .with_timer(timer)
        .with_target(true)
        .with_level(true)
        .with_filter(filter_fn(|metadata| {
            // Filter out overly verbose internal query logs
            !metadata.target().starts_with("sqlx::")
        }))
}

/// Initialize console logging: pretty format output to stdout.
///
/// This function calls `try_init()` and can only be called once per process.
pub fn console_tracing(crates: Vec<String>) -> anyhow::Result<()> {
    let env_filter = build_debug_filter(crates);

    tracing_subscriber::registry()
        .with(env_filter)
        .with(build_console_layer())
        .try_init()?;

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    // All tests accessing RUST_LOG hold crate::TEST_ENV_MUTEX, serialized together with the tests
    // in the file_tracing module, to avoid concurrent read/write of process environment variables
    // (unsafe) causing UB.

    #[test]
    fn build_debug_filter_creates_valid_filter() {
        // SAFETY: Reading the RUST_LOG env var must be serialized with the tests that write RUST_LOG
        // in the same binary; holding the crate::TEST_ENV_MUTEX lock avoids concurrent read/write of
        // the process environment variable (UB).
        let _lock = crate::TEST_ENV_MUTEX.lock().unwrap();
        let filter = build_debug_filter(Vec::new());
        // Verify the filter can be created without panicking
        let _ = format!("{:?}", filter);
    }

    #[test]
    fn build_debug_filter_respects_env_var() {
        // SAFETY: From Rust 2024, modifying process environment variables is unsafe. Holding the
        // crate::TEST_ENV_MUTEX lock ensures the write/restore of RUST_LOG in this test runs
        // serialized with other tests reading RUST_LOG in the same binary, avoiding race conditions.
        let _lock = crate::TEST_ENV_MUTEX.lock().unwrap();
        unsafe {
            std::env::set_var("RUST_LOG", "warn");
        }
        let filter = build_debug_filter(vec!["commons".into()]);
        let _ = format!("{:?}", filter);
        // SAFETY: Same as above; restore the environment variable set by this test.
        unsafe {
            std::env::remove_var("RUST_LOG");
        }
    }

    #[test]
    fn console_tracing_initialization() {
        // Verify console_tracing can initialize normally.
        // Hold the crate::TEST_ENV_MUTEX lock: console_tracing reads the RUST_LOG env var internally
        // and must be serialized with tests that write RUST_LOG.
        let _lock = crate::TEST_ENV_MUTEX.lock().unwrap();
        let result = console_tracing(vec!["commons".into()]);
        assert!(result.is_ok());
    }
}
