//! Console logging initialization: pretty-format output with colors, file name, and line number.
//!
//! Timestamps are RFC 3339 with the offset resolved in [`crate::timestamp`]; colors are enabled
//! only when stdout is a terminal. The log level is decided by the [`EnvFilter`] built in this
//! module; see [`InitOptions`](crate::InitOptions) for the authoritative description of how
//! `crates` and `RUST_LOG` interact.

use std::io::IsTerminal;
use time::UtcOffset;
use tracing_subscriber::{EnvFilter, Layer, fmt::format::FmtSpan, prelude::*};

/// Build the project-level log filter
///
/// Project crates passed via the `crates` parameter by the caller use the `debug` level,
/// other dependency crates use the `info` level.
///
/// The `RUST_LOG` environment variable is layered **on top** of those directives instead of
/// replacing them. Because `EnvFilter` lets a later directive win for the target it names, this
/// means:
///
/// - `RUST_LOG=hyper=warn` only quiets `hyper`; the crates passed via `crates` stay at `debug`.
/// - `RUST_LOG=my_crate=trace` overrides the level of an individual project crate.
/// - a bare level such as `RUST_LOG=warn` replaces the global default (`info`), so it raises the
///   threshold for every crate that has no target-specific directive.
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
    EnvFilter::new(debug_directives(&crates))
}

/// Build the directive list shared by the console layer and the file layer of `Test` mode.
///
/// Reads `RUST_LOG` and layers it on top of the `crates` defaults; see [`build_debug_filter`] for
/// the resulting semantics.
pub(crate) fn debug_directives(crates: &[String]) -> String {
    build_directives(crates, std::env::var("RUST_LOG").ok().as_deref())
}

/// Build the comma-separated directive list handed to [`EnvFilter`].
///
/// The `crates` are emitted as `{crate}=debug` and everything else defaults to `info`. Directives
/// read from `rust_log` are appended afterwards so that they take precedence over those defaults
/// (for the same target), while targets they do not mention keep the level derived from `crates`.
///
/// Entries may be separated by newlines as well as by commas (the latter are handled by
/// `EnvFilter` itself), and blank entries are dropped so a trailing newline does not produce an
/// invalid directive.
fn build_directives(crates: &[String], rust_log: Option<&str>) -> String {
    let mut directives: Vec<String> = Vec::with_capacity(crates.len() + 1);

    // Build the debug-level directive for project crates
    for crate_name in crates {
        match sanitize_crate_name(crate_name) {
            Some(name) => directives.push(format!("{name}=debug")),
            None => eprintln!(
                "Ignoring invalid crate name {crate_name:?} in InitOptions::crates: expected a \
                 plain crate name"
            ),
        }
    }

    // Other crates default to info level
    directives.push(crate::DEFAULT_LOG_LEVEL.to_string());

    if let Some(rust_log) = rust_log {
        directives.extend(
            rust_log
                .split(['\n', '\r'])
                .map(str::trim)
                .filter(|directive| !directive.is_empty())
                .map(str::to_string),
        );
    }

    directives.join(",")
}

/// Normalize a caller-provided crate name, rejecting values that cannot form a valid directive.
///
/// [`EnvFilter::new`] parses one comma-separated string and silently *skips* directives it cannot
/// parse, so a name like `"my,crate"` would quietly lose its `debug` level. Returning `None` lets
/// [`build_directives`] report the problem instead.
fn sanitize_crate_name(crate_name: &str) -> Option<&str> {
    let name = crate_name.trim();
    let has_separator =
        name.contains([',', '=', '[', ']', '{', '}']) || name.contains(char::is_whitespace);
    (!name.is_empty() && !has_separator).then_some(name)
}

/// Build the console log layer (pretty format) using the given timestamp offset
///
/// The layer formats and styles records but never filters them: whether a record is emitted is
/// decided by the [`EnvFilter`] attached through [`Layer::with_filter`] (see
/// [`build_debug_filter`]). That keeps the library free of assumptions about which dependency
/// crates the caller considers too verbose — use a per-target `RUST_LOG` directive such as
/// `RUST_LOG=sqlx=warn` instead.
///
/// Timestamps are RFC 3339 with `time_offset`, using the same timer as the file layer so both
/// outputs are directly comparable.
///
/// Colors are enabled only when stdout is a terminal, so redirected output and CI stay clean and
/// `NO_COLOR` is honoured. Target and level are printed by default; file, line number, thread ids
/// and thread names are enabled explicitly here.
pub(crate) fn build_console_layer<S>(time_offset: UtcOffset) -> impl Layer<S>
where
    S: tracing::Subscriber + for<'a> tracing_subscriber::registry::LookupSpan<'a>,
{
    tracing_subscriber::fmt::layer()
        .pretty()
        .with_ansi(std::io::stdout().is_terminal())
        .with_file(true)
        .with_line_number(true)
        .with_timer(crate::timestamp::rfc3339_timer(time_offset))
        .with_thread_ids(true)
        .with_thread_names(true)
        .with_span_events(FmtSpan::CLOSE)
}

/// Initialize console logging: pretty format output to stdout.
///
/// This function calls `try_init()` and can only be called once per process. `crates` lists the
/// project crates that should log at `debug`; the only other environment input is the timestamp
/// offset, read from the prefixed `LOG_TIME_OFFSET` variable (see
/// [`InitOptions::env_prefix`](crate::InitOptions::env_prefix)) and otherwise taken from the
/// machine's own time zone ([`crate::DEFAULT_TIME_OFFSET`] is the last resort). `RUST_LOG` stays
/// unprefixed, because the whole Rust logging ecosystem shares it.
pub fn console_tracing(env_prefix: &str, crates: Vec<String>) -> anyhow::Result<()> {
    console_tracing_with_offset(env_prefix, crates, None)
}

/// [`console_tracing`] with an explicit timestamp offset override; used by [`crate::init`] so that
/// [`InitOptions::time_offset`](crate::InitOptions::time_offset) reaches the layer.
pub(crate) fn console_tracing_with_offset(
    env_prefix: &str,
    crates: Vec<String>,
    time_offset: Option<&str>,
) -> anyhow::Result<()> {
    let env_filter = build_debug_filter(crates);
    let offset = crate::timestamp::resolve_time_offset(env_prefix, time_offset);
    let layer = build_console_layer(offset);

    crate::install_subscriber(tracing_subscriber::registry().with(layer.with_filter(env_filter)))?;

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    // All tests accessing RUST_LOG hold crate::TEST_ENV_MUTEX, serialized together with the tests
    // in the file_tracing module, to avoid concurrent read/write of process environment variables
    // (unsafe) causing UB.

    // The `build_directives` / `sanitize_crate_name` tests exercise pure string logic and never
    // touch the environment.

    #[test]
    fn sanitize_crate_name_accepts_plain_names_and_rejects_separators() {
        assert_eq!(sanitize_crate_name(" commons "), Some("commons"));
        for value in ["", "  ", "my,crate", "my=crate", "my[crate]", "my crate"] {
            assert_eq!(sanitize_crate_name(value), None, "value: {value:?}");
        }
    }

    #[test]
    fn build_directives_skips_invalid_crate_names() {
        // A name that would break the directive list is dropped rather than silently truncating
        // it, so the remaining crates keep their `debug` level.
        let directives =
            build_directives(&["commons".to_string(), "tools,hyper".to_string()], None);
        assert_eq!(directives, "commons=debug,info");
    }

    #[test]
    fn build_directives_defaults_crates_to_debug_and_the_rest_to_info() {
        let directives = build_directives(&["commons".to_string(), "tools".to_string()], None);
        assert_eq!(directives, "commons=debug,tools=debug,info");
    }

    #[test]
    fn build_directives_appends_env_directives_after_the_built_in_ones() {
        // Appended last: EnvFilter lets a later directive win for the target it names.
        let directives = build_directives(&["commons".to_string()], Some("hyper=warn"));
        assert_eq!(directives, "commons=debug,info,hyper=warn");
    }

    #[test]
    fn build_directives_accepts_newlines_and_skips_blank_entries() {
        let directives = build_directives(&[], Some("\ncommons=debug\n\nhyper=warn\n"));
        assert_eq!(directives, "info,commons=debug,hyper=warn");
    }

    #[test]
    fn build_debug_filter_layers_env_var_over_crates() {
        // SAFETY: From Rust 2024, modifying process environment variables is unsafe. Holding the
        // crate::TEST_ENV_MUTEX lock ensures the write/restore of RUST_LOG in this test runs
        // serialized with other tests reading RUST_LOG in the same binary, avoiding race conditions.
        let _lock = crate::TEST_ENV_MUTEX.lock().unwrap();

        // A bare level in RUST_LOG replaces the global default only; the caller's crates keep debug.
        unsafe {
            std::env::set_var("RUST_LOG", "warn");
        }
        let rendered = build_debug_filter(vec!["commons".into()]).to_string();
        assert!(rendered.contains("commons=debug"), "rendered: {rendered}");
        assert!(rendered.contains("warn"), "rendered: {rendered}");

        // An env directive for the same target wins over the level derived from `crates`.
        unsafe {
            std::env::set_var("RUST_LOG", "commons=error");
        }
        let rendered = build_debug_filter(vec!["commons".into()]).to_string();
        assert!(rendered.contains("commons=error"), "rendered: {rendered}");
        assert!(!rendered.contains("commons=debug"), "rendered: {rendered}");

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
        let result = console_tracing("NGY_TEST_", vec!["commons".into()]);
        assert!(result.is_ok());
    }
}
